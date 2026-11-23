package fakes3

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
)

const defaultDownloadURL = "https://github.com/AkaraChen/fakes3/releases/download"

// maxArchive bounds a downloaded archive; a release archive is a few MB.
const maxArchive = 256 << 20

var downloadMu sync.Mutex

func findBinary(ctx context.Context, opts Options) (string, error) {
	if opts.Binary != "" {
		return opts.Binary, nil
	}
	if p := os.Getenv("FAKES3_BINARY"); p != "" {
		return p, nil
	}
	if p, err := exec.LookPath("fakes3"); err == nil {
		return p, nil
	}
	version := firstNonEmpty(opts.Version, os.Getenv("FAKES3_VERSION"), Version)
	cacheDir := firstNonEmpty(opts.CacheDir, os.Getenv("FAKES3_CACHE_DIR"))
	if cacheDir == "" {
		dir, err := os.UserCacheDir()
		if err != nil {
			dir = os.TempDir()
		}
		cacheDir = filepath.Join(dir, "fakes3")
	}
	base := firstNonEmpty(opts.DownloadURL, os.Getenv("FAKES3_DOWNLOAD_URL"), defaultDownloadURL)
	return download(ctx, http.DefaultClient, base, version, cacheDir, runtime.GOOS, runtime.GOARCH)
}

// asset returns the release archive name for a platform.
func asset(goos, goarch string) (string, error) {
	var triple string
	switch goos + "/" + goarch {
	case "linux/amd64":
		triple = "x86_64-unknown-linux-musl"
	case "linux/arm64":
		triple = "aarch64-unknown-linux-musl"
	case "darwin/amd64":
		triple = "x86_64-apple-darwin"
	case "darwin/arm64":
		triple = "aarch64-apple-darwin"
	case "windows/amd64":
		return "fakes3-x86_64-pc-windows-msvc.zip", nil
	default:
		return "", fmt.Errorf("no prebuilt fakes3 for %s/%s; build it with `cargo install fakes3` and set FAKES3_BINARY", goos, goarch)
	}
	return "fakes3-" + triple + ".tar.gz", nil
}

// download returns the cached binary for version, fetching and verifying it first if needed.
func download(ctx context.Context, client *http.Client, base, version, cacheDir, goos, goarch string) (string, error) {
	version = strings.TrimPrefix(version, "v")
	name, err := asset(goos, goarch)
	if err != nil {
		return "", err
	}
	exe := "fakes3"
	if goos == "windows" {
		exe += ".exe"
	}
	dir := filepath.Join(cacheDir, "v"+version, strings.TrimSuffix(strings.TrimSuffix(name, ".zip"), ".tar.gz"))
	bin := filepath.Join(dir, exe)
	if isFile(bin) {
		return bin, nil
	}
	downloadMu.Lock()
	defer downloadMu.Unlock()
	if isFile(bin) {
		return bin, nil
	}

	url := strings.TrimSuffix(base, "/") + "/v" + version + "/" + name
	sumFile, err := fetch(ctx, client, url+".sha256", 4<<10)
	if err != nil {
		return "", fmt.Errorf("download fakes3 %s checksum: %w", version, err)
	}
	want, err := parseSHA256(sumFile)
	if err != nil {
		return "", fmt.Errorf("%s.sha256: %w", url, err)
	}
	archive, err := fetch(ctx, client, url, maxArchive)
	if err != nil {
		return "", fmt.Errorf("download fakes3 %s: %w", version, err)
	}
	sum := sha256.Sum256(archive)
	if got := hex.EncodeToString(sum[:]); got != want {
		return "", fmt.Errorf("checksum mismatch for %s: expected sha256 %s, got %s", url, want, got)
	}
	var data []byte
	if strings.HasSuffix(name, ".zip") {
		data, err = fromZip(archive, exe)
	} else {
		data, err = fromTarGz(archive, exe)
	}
	if err != nil {
		return "", fmt.Errorf("%s: %w", url, err)
	}

	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	tmp, err := os.CreateTemp(dir, exe+".*.tmp")
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp.Name())
	_, err = tmp.Write(data)
	if cerr := tmp.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Chmod(tmp.Name(), 0o755)
	}
	if err != nil {
		return "", err
	}
	// Another process may have installed the same binary meanwhile; on Windows it can't be
	// replaced while running, and either copy is the same file.
	if err := os.Rename(tmp.Name(), bin); err != nil && !isFile(bin) {
		return "", err
	}
	return bin, nil
}

func fetch(ctx context.Context, client *http.Client, url string, limit int64) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("GET %s: %s", url, resp.Status)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(body)) > limit {
		return nil, fmt.Errorf("GET %s: response larger than %d bytes", url, limit)
	}
	return body, nil
}

// parseSHA256 reads a sha256sum-style line: the hex digest, optionally followed by a file name.
func parseSHA256(b []byte) (string, error) {
	fields := strings.Fields(string(b))
	if len(fields) == 0 {
		return "", errors.New("empty checksum file")
	}
	sum := strings.ToLower(fields[0])
	if len(sum) != 64 {
		return "", fmt.Errorf("not a SHA-256 checksum: %q", fields[0])
	}
	if _, err := hex.DecodeString(sum); err != nil {
		return "", fmt.Errorf("not a SHA-256 checksum: %q", fields[0])
	}
	return sum, nil
}

func fromTarGz(archive []byte, exe string) ([]byte, error) {
	gz, err := gzip.NewReader(bytes.NewReader(archive))
	if err != nil {
		return nil, err
	}
	tr := tar.NewReader(gz)
	for {
		h, err := tr.Next()
		if err == io.EOF {
			return nil, fmt.Errorf("no %s in archive", exe)
		}
		if err != nil {
			return nil, err
		}
		if h.Typeflag == tar.TypeReg && path.Base(h.Name) == exe {
			return io.ReadAll(io.LimitReader(tr, maxArchive))
		}
	}
}

func fromZip(archive []byte, exe string) ([]byte, error) {
	zr, err := zip.NewReader(bytes.NewReader(archive), int64(len(archive)))
	if err != nil {
		return nil, err
	}
	for _, f := range zr.File {
		if !f.FileInfo().IsDir() && path.Base(strings.ReplaceAll(f.Name, `\`, "/")) == exe {
			r, err := f.Open()
			if err != nil {
				return nil, err
			}
			defer r.Close()
			return io.ReadAll(io.LimitReader(r, maxArchive))
		}
	}
	return nil, fmt.Errorf("no %s in archive", exe)
}

func isFile(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.Mode().IsRegular()
}

func firstNonEmpty(vs ...string) string {
	for _, v := range vs {
		if v != "" {
			return v
		}
	}
	return ""
}
