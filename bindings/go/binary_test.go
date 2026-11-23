package fakes3

import (
	"archive/tar"
	"archive/zip"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
)

// release builds a fake release archive for the current platform, laid out like the real ones.
func release(t *testing.T, binary []byte) (name string, archive []byte) {
	t.Helper()
	name, err := asset(runtime.GOOS, runtime.GOARCH)
	if err != nil {
		t.Skip(err)
	}
	dir := strings.TrimSuffix(strings.TrimSuffix(name, ".zip"), ".tar.gz")
	var buf bytes.Buffer
	if strings.HasSuffix(name, ".zip") {
		zw := zip.NewWriter(&buf)
		for _, f := range []string{"README.md", "fakes3.exe"} {
			w, _ := zw.Create(dir + "/" + f)
			w.Write(binary)
		}
		zw.Close()
	} else {
		gz := gzip.NewWriter(&buf)
		tw := tar.NewWriter(gz)
		tw.WriteHeader(&tar.Header{Name: dir + "/", Typeflag: tar.TypeDir, Mode: 0o755})
		for _, f := range []string{"README.md", "fakes3"} {
			tw.WriteHeader(&tar.Header{Name: dir + "/" + f, Typeflag: tar.TypeReg, Mode: 0o755, Size: int64(len(binary))})
			tw.Write(binary)
		}
		tw.Close()
		gz.Close()
	}
	return name, buf.Bytes()
}

// serve publishes an archive and its checksum file under /v1.2.3/, counting downloads.
func serve(t *testing.T, name string, archive []byte, sumFile string) (url string, downloads *atomic.Int32) {
	t.Helper()
	downloads = new(atomic.Int32)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1.2.3/" + name:
			downloads.Add(1)
			w.Write(archive)
		case "/v1.2.3/" + name + ".sha256":
			if sumFile == "" {
				http.NotFound(w, r)
				return
			}
			w.Write([]byte(sumFile))
		default:
			http.NotFound(w, r)
		}
	}))
	t.Cleanup(srv.Close)
	return srv.URL, downloads
}

func sha(b []byte) string {
	s := sha256.Sum256(b)
	return hex.EncodeToString(s[:])
}

func TestDownloadVerifiesAndCaches(t *testing.T) {
	binary := []byte("the fakes3 binary")
	name, archive := release(t, binary)
	url, downloads := serve(t, name, archive, sha(archive)+"  "+name+"\n")
	cache := t.TempDir()

	for range 2 {
		bin, err := download(context.Background(), http.DefaultClient, url, "v1.2.3", cache, runtime.GOOS, runtime.GOARCH)
		if err != nil {
			t.Fatal(err)
		}
		got, err := os.ReadFile(bin)
		if err != nil || !bytes.Equal(got, binary) {
			t.Fatalf("installed binary = %q, %v", got, err)
		}
		if !strings.HasPrefix(bin, filepath.Join(cache, "v1.2.3")) {
			t.Fatalf("binary installed at %s, outside the cache", bin)
		}
	}
	if n := downloads.Load(); n != 1 {
		t.Fatalf("archive downloaded %d times, want once", n)
	}
}

func TestDownloadRejectsChecksumMismatch(t *testing.T) {
	name, archive := release(t, []byte("the fakes3 binary"))
	tampered := append([]byte(nil), archive...)
	tampered[len(tampered)/2] ^= 0xff
	url, _ := serve(t, name, tampered, sha(archive)+"  "+name+"\n")
	cache := t.TempDir()

	_, err := download(context.Background(), http.DefaultClient, url, "1.2.3", cache, runtime.GOOS, runtime.GOARCH)
	if err == nil || !strings.Contains(err.Error(), "checksum mismatch") {
		t.Fatalf("err = %v, want a checksum mismatch", err)
	}
	// Nothing is installed, so the next run downloads again instead of using a bad binary.
	var files []string
	filepath.WalkDir(cache, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			files = append(files, p)
		}
		return nil
	})
	if len(files) != 0 {
		t.Fatalf("cache has %v after a failed download", files)
	}
}

func TestDownloadRequiresChecksum(t *testing.T) {
	name, archive := release(t, []byte("the fakes3 binary"))
	for _, sumFile := range []string{"", "not-a-checksum\n"} {
		url, downloads := serve(t, name, archive, sumFile)
		_, err := download(context.Background(), http.DefaultClient, url, "1.2.3", t.TempDir(), runtime.GOOS, runtime.GOARCH)
		if err == nil {
			t.Fatalf("checksum file %q: download succeeded", sumFile)
		}
		if downloads.Load() != 0 {
			t.Fatalf("checksum file %q: archive fetched without a usable checksum", sumFile)
		}
	}
}

func TestAssetNames(t *testing.T) {
	for platform, want := range map[string]string{
		"linux/amd64":   "fakes3-x86_64-unknown-linux-musl.tar.gz",
		"linux/arm64":   "fakes3-aarch64-unknown-linux-musl.tar.gz",
		"darwin/amd64":  "fakes3-x86_64-apple-darwin.tar.gz",
		"darwin/arm64":  "fakes3-aarch64-apple-darwin.tar.gz",
		"windows/amd64": "fakes3-x86_64-pc-windows-msvc.zip",
	} {
		goos, goarch, _ := strings.Cut(platform, "/")
		if got, err := asset(goos, goarch); got != want || err != nil {
			t.Errorf("%s: %q, %v; want %q", platform, got, err, want)
		}
	}
	if _, err := asset("windows", "arm64"); err == nil {
		t.Error("windows/arm64 has no release asset")
	}
}

func TestParseSHA256(t *testing.T) {
	sum := strings.Repeat("ab", 32)
	for _, in := range []string{sum, sum + "\n", sum + "  fakes3.tar.gz\n", sum + " *fakes3.zip\r\n", strings.ToUpper(sum)} {
		if got, err := parseSHA256([]byte(in)); got != sum || err != nil {
			t.Errorf("%q: %q, %v", in, got, err)
		}
	}
	for _, in := range []string{"", "abc", strings.Repeat("zz", 32)} {
		if _, err := parseSHA256([]byte(in)); err == nil {
			t.Errorf("%q accepted", in)
		}
	}
}

func TestChildEnvDropsFakeS3Settings(t *testing.T) {
	t.Setenv("FAKES3_AUTH", "true")
	t.Setenv("FAKES3_BINARY", "/x")
	for _, kv := range childEnv() {
		if strings.HasPrefix(strings.ToUpper(kv), "FAKES3_") {
			t.Errorf("child env has %s", kv)
		}
	}
}
