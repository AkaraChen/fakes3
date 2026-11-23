// Package fakes3 starts fakes3, an in-memory S3 server, from Go tests.
//
// Each server is a separate fakes3 process on a free port, with its own data and clock, so
// parallel tests don't interfere with each other. It starts in a few milliseconds and needs no
// Docker:
//
//	func TestUpload(t *testing.T) {
//		t.Parallel()
//		srv := fakes3.Start(t) // stopped when the test ends
//		client := s3.New(s3.Options{ // aws-sdk-go-v2; any S3 client works
//			BaseEndpoint: aws.String(srv.Endpoint),
//			Region:       srv.Region,
//			UsePathStyle: true,
//			Credentials:  credentials.NewStaticCredentialsProvider(srv.AccessKey, srv.SecretKey, ""),
//		})
//		// ...
//	}
//
// The fakes3 binary is taken from, in order: [Options.Binary], the FAKES3_BINARY environment
// variable, fakes3 on PATH, and finally a download of release [Version] from GitHub into the user
// cache directory, checked against the SHA-256 checksum published with the release.
package fakes3

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// Version is the fakes3 release downloaded when no binary is configured. Override it with
// [Options.Version] or the FAKES3_VERSION environment variable.
const Version = "0.1.4"

// Region is the region to configure S3 clients with. fakes3 accepts any region; this is the one
// it reports for buckets created without a location constraint.
const Region = "us-east-1"

// User is an additional account. Each account owns the buckets and objects it creates. AccountID
// and CanonicalID are optional and generated when empty.
type User struct {
	Name        string
	AccessKey   string
	SecretKey   string
	AccountID   string
	CanonicalID string
}

// Options configures a server. The zero value is a server on 127.0.0.1 with a random free port,
// any credentials accepted, and a binary found or downloaded as described in the package
// documentation.
type Options struct {
	// Host is the address to listen on. Default "127.0.0.1".
	Host string
	// AccessKey and SecretKey are the default account's credentials, checked only with Auth.
	// Default "fakes3" and "fakes3secret".
	AccessKey string
	SecretKey string
	// Auth verifies request signatures. By default any credentials work: a signed request acts
	// as the account its access key belongs to (the default account if none does), and an
	// unsigned request is anonymous, subject to ACLs, bucket policies and Block Public Access.
	Auth bool
	// Users are more accounts.
	Users []User
	// BucketDefaults is the settings new buckets start with: "legacy" (default), or "aws" for
	// what AWS applies today (ACLs disabled, Block Public Access on, SSE-S3, SSE-C blocked).
	BucketDefaults string
	// MemoryLimit caps object data: "512MiB", "2GiB", "unlimited", or "auto" (default, 75% of the
	// container's or machine's memory). Writes past it fail with HTTP 507.
	MemoryLimit string
	// Domains are base domains for virtual-host addressing (<bucket>.<domain>). Default
	// "localhost". Path-style addressing always works.
	Domains []string
	// DisableRequestLog stops recording requests for [Server.Requests].
	DisableRequestLog bool
	// RequestLogCapacity is how many requests the log keeps. Default 10000.
	RequestLogCapacity int
	// Args are extra command-line arguments for fakes3.
	Args []string

	// Binary is the path of the fakes3 executable. When empty, FAKES3_BINARY, then fakes3 on
	// PATH, then a downloaded release are used.
	Binary string
	// Version is the release to download, such as "0.1.4". Default: FAKES3_VERSION, or [Version].
	Version string
	// CacheDir is where downloaded binaries are kept. Default: FAKES3_CACHE_DIR, or "fakes3" in
	// the user cache directory ([os.UserCacheDir]).
	CacheDir string
	// DownloadURL is the base URL releases are downloaded from; the archive is fetched from
	// <DownloadURL>/v<version>/<asset>. Default: FAKES3_DOWNLOAD_URL, or
	// https://github.com/AkaraChen/fakes3/releases/download. Use it for a mirror.
	DownloadURL string
	// Stderr receives the server's log output. By default it is kept only to explain a failed
	// start.
	Stderr io.Writer
	// StartTimeout bounds how long starting may take, not counting a download. Default 30s.
	StartTimeout time.Duration
}

// Server is a running fakes3 process.
type Server struct {
	// Endpoint is the server's URL, such as "http://127.0.0.1:53124", ready for an S3 client
	// with path-style addressing.
	Endpoint string
	// Host and Port are where the server listens.
	Host string
	Port int
	// AccessKey and SecretKey are the default account's credentials.
	AccessKey string
	SecretKey string
	// Region is the region to configure clients with, always [Region].
	Region string

	cmd      *exec.Cmd
	done     chan struct{}
	guard    guard
	http     *http.Client
	stopOnce sync.Once
	clock    Clock
	requests RequestLog
}

// Start starts a server with default options and stops it when the test and its subtests finish.
// It fails the test if the server cannot start.
func Start(tb testing.TB) *Server {
	tb.Helper()
	return StartWith(tb, Options{})
}

// StartWith is [Start] with options.
func StartWith(tb testing.TB, opts Options) *Server {
	tb.Helper()
	s, err := New(context.Background(), opts)
	if err != nil {
		tb.Fatalf("fakes3: %v", err)
	}
	tb.Cleanup(func() { _ = s.Close() })
	return s
}

// New starts a server outside of a test, for example in TestMain. Call [Server.Close] to stop it.
// The context bounds the binary download and the start; cancelling it later has no effect.
func New(ctx context.Context, opts Options) (*Server, error) {
	bin, err := findBinary(ctx, opts)
	if err != nil {
		return nil, err
	}
	accessKey := orDefault(opts.AccessKey, "fakes3")
	secretKey := orDefault(opts.SecretKey, "fakes3secret")
	args := []string{
		"--host", orDefault(opts.Host, "127.0.0.1"),
		"--port", "0",
		"--access-key", accessKey,
		"--secret-key", secretKey,
	}
	if opts.Auth {
		args = append(args, "--auth")
	}
	for _, u := range opts.Users {
		spec := u.Name + ":" + u.AccessKey + ":" + u.SecretKey
		if u.AccountID != "" || u.CanonicalID != "" {
			spec += ":" + u.AccountID
		}
		if u.CanonicalID != "" {
			spec += ":" + u.CanonicalID
		}
		args = append(args, "--user", spec)
	}
	if opts.BucketDefaults != "" {
		args = append(args, "--bucket-defaults", opts.BucketDefaults)
	}
	if opts.MemoryLimit != "" {
		args = append(args, "--memory-limit", opts.MemoryLimit)
	}
	for _, d := range opts.Domains {
		args = append(args, "--domain", d)
	}
	if opts.DisableRequestLog {
		args = append(args, "--no-request-log")
	}
	if opts.RequestLogCapacity > 0 {
		args = append(args, "--request-log-capacity", strconv.Itoa(opts.RequestLogCapacity))
	}
	args = append(args, opts.Args...)

	timeout := opts.StartTimeout
	if timeout <= 0 {
		timeout = 30 * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	cmd := exec.Command(bin, args...)
	cmd.Env = childEnv()
	stderr := &tailBuffer{max: 64 << 10}
	if opts.Stderr != nil {
		cmd.Stderr = io.MultiWriter(stderr, opts.Stderr)
	} else {
		cmd.Stderr = stderr
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	prepare(cmd)
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("start %s: %w", bin, err)
	}
	g := protect(cmd)

	lines := make(chan string, 1)
	out := bufio.NewReader(stdout)
	go func() {
		line, _ := out.ReadString('\n')
		lines <- strings.TrimSpace(line)
	}()
	var line string
	timedOut := false
	select {
	case line = <-lines:
	case <-ctx.Done():
		timedOut = true
	}
	if !strings.HasPrefix(line, "http://") {
		g.release()
		_ = cmd.Process.Kill()
		if timedOut {
			<-lines // the pipe closes once the process is gone
		}
		_ = cmd.Wait()
		msg := strings.TrimSpace(stderr.String())
		if timedOut {
			return nil, fmt.Errorf("%s did not start within %v: %s", bin, timeout, msg)
		}
		if line != "" {
			msg = strings.TrimSpace(line + "\n" + msg)
		}
		return nil, fmt.Errorf("%s exited during start: %s", bin, msg)
	}
	u, err := url.Parse(line)
	if err != nil {
		g.release()
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
		return nil, fmt.Errorf("unexpected endpoint %q: %w", line, err)
	}
	port, _ := strconv.Atoi(u.Port())

	s := &Server{
		Endpoint:  line,
		Host:      u.Hostname(),
		Port:      port,
		AccessKey: accessKey,
		SecretKey: secretKey,
		Region:    Region,
		cmd:       cmd,
		done:      make(chan struct{}),
		guard:     g,
		// The control plane is on loopback: never send it through a proxy.
		http: &http.Client{
			Transport: &http.Transport{Proxy: nil, MaxIdleConnsPerHost: 4},
			Timeout:   30 * time.Second,
		},
	}
	s.clock.s = s
	s.requests.s = s
	go func() {
		_, _ = io.Copy(io.Discard, out)
		_ = cmd.Wait()
		close(s.done)
	}()
	return s, nil
}

// Close stops the server and waits for the process to exit; the port is free when it returns.
// Calling it more than once is safe.
func (s *Server) Close() error {
	s.stopOnce.Do(func() {
		s.guard.release()
		_ = s.cmd.Process.Kill()
		<-s.done
		s.http.CloseIdleConnections()
	})
	return nil
}

func orDefault(v, def string) string {
	if v == "" {
		return def
	}
	return v
}

// childEnv is the environment without fakes3's own FAKES3_* settings, so a variable in the
// developer's shell can't change how the test servers behave.
func childEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		if len(kv) >= 7 && strings.EqualFold(kv[:7], "FAKES3_") {
			continue
		}
		env = append(env, kv)
	}
	return env
}

// tailBuffer keeps the last max bytes written to it.
type tailBuffer struct {
	mu  sync.Mutex
	buf []byte
	max int
}

func (b *tailBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.buf = append(b.buf, p...)
	if len(b.buf) > b.max {
		b.buf = append([]byte(nil), b.buf[len(b.buf)-b.max:]...)
	}
	return len(p), nil
}

func (b *tailBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return string(b.buf)
}
