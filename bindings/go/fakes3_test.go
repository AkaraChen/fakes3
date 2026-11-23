package fakes3_test

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"

	fakes3 "github.com/AkaraChen/fakes3/bindings/go"
)

func client(s *fakes3.Server) *s3.Client {
	return s3.New(s3.Options{
		BaseEndpoint: aws.String(s.Endpoint),
		Region:       s.Region,
		UsePathStyle: true,
		Credentials:  credentials.NewStaticCredentialsProvider(s.AccessKey, s.SecretKey, ""),
	})
}

func put(t *testing.T, c *s3.Client, bucket, key, body string) {
	t.Helper()
	_, err := c.PutObject(context.Background(), &s3.PutObjectInput{
		Bucket: aws.String(bucket), Key: aws.String(key), Body: strings.NewReader(body),
	})
	if err != nil {
		t.Fatal(err)
	}
}

func createBucket(t *testing.T, c *s3.Client, bucket string) {
	t.Helper()
	if _, err := c.CreateBucket(context.Background(), &s3.CreateBucketInput{Bucket: aws.String(bucket)}); err != nil {
		t.Fatal(err)
	}
}

func keys(t *testing.T, c *s3.Client, bucket string) []string {
	t.Helper()
	out, err := c.ListObjectsV2(context.Background(), &s3.ListObjectsV2Input{Bucket: aws.String(bucket)})
	if err != nil {
		t.Fatal(err)
	}
	var ks []string
	for _, o := range out.Contents {
		ks = append(ks, *o.Key)
	}
	return ks
}

// Every test gets its own server, so the same bucket name in parallel tests never collides.
func TestParallelServersAreIndependent(t *testing.T) {
	for i := range 8 {
		t.Run(fmt.Sprint(i), func(t *testing.T) {
			t.Parallel()
			s := fakes3.Start(t)
			c := client(s)
			createBucket(t, c, "demo")
			key := fmt.Sprintf("only-%d.txt", i)
			put(t, c, "demo", key, "hi")
			if got := keys(t, c, "demo"); len(got) != 1 || got[0] != key {
				t.Fatalf("keys = %v, want [%s]", got, key)
			}
		})
	}
}

func TestPresignedURLExpiresWithClock(t *testing.T) {
	t.Parallel()
	s := fakes3.Start(t)
	c := client(s)
	createBucket(t, c, "demo")
	put(t, c, "demo", "report.csv", "a,b\n")

	req, err := s3.NewPresignClient(c).PresignGetObject(context.Background(),
		&s3.GetObjectInput{Bucket: aws.String("demo"), Key: aws.String("report.csv")},
		s3.WithPresignExpires(time.Minute))
	if err != nil {
		t.Fatal(err)
	}
	get := func() (int, string) {
		resp, err := http.Get(req.URL)
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		b, _ := io.ReadAll(resp.Body)
		return resp.StatusCode, string(b)
	}
	if code, body := get(); code != 200 || body != "a,b\n" {
		t.Fatalf("before expiry: %d %q", code, body)
	}
	if err := s.Clock().Advance(2 * time.Minute); err != nil {
		t.Fatal(err)
	}
	if code, body := get(); code != 403 || !strings.Contains(body, "expired") {
		t.Fatalf("after expiry: %d %q", code, body)
	}
}

func TestClock(t *testing.T) {
	t.Parallel()
	s := fakes3.Start(t)
	at := time.Date(2030, 1, 2, 3, 4, 5, 0, time.UTC)
	if err := s.Clock().FreezeAt(at); err != nil {
		t.Fatal(err)
	}
	if err := s.Clock().Advance(31 * 24 * time.Hour); err != nil {
		t.Fatal(err)
	}
	st, err := s.Clock().State()
	if err != nil {
		t.Fatal(err)
	}
	if want := at.AddDate(0, 0, 31); !st.Now.Equal(want) || !st.Frozen || !st.Shifted {
		t.Fatalf("clock = %+v, want frozen at %v", st, want)
	}

	// Objects written now carry the server time.
	c := client(s)
	createBucket(t, c, "demo")
	put(t, c, "demo", "a", "1")
	head, err := c.HeadObject(context.Background(), &s3.HeadObjectInput{Bucket: aws.String("demo"), Key: aws.String("a")})
	if err != nil {
		t.Fatal(err)
	}
	if !head.LastModified.Equal(st.Now) {
		t.Fatalf("LastModified = %v, want %v", head.LastModified, st.Now)
	}

	if err := s.Clock().Reset(); err != nil {
		t.Fatal(err)
	}
	if st, _ := s.Clock().State(); st.Frozen || st.Shifted {
		t.Fatalf("after reset: %+v", st)
	}
}

func TestRequestLog(t *testing.T) {
	t.Parallel()
	s := fakes3.Start(t)
	c := client(s)
	createBucket(t, c, "demo")
	put(t, c, "demo", "a.txt", "1")
	put(t, c, "demo", "b.txt", "22")
	_, err := c.GetObject(context.Background(), &s3.GetObjectInput{Bucket: aws.String("demo"), Key: aws.String("missing")})
	if err == nil {
		t.Fatal("GetObject of a missing key succeeded")
	}

	puts, err := s.Requests().List(fakes3.RequestFilter{Operation: "PutObject"})
	if err != nil {
		t.Fatal(err)
	}
	if len(puts) != 2 || puts[0].Key != "a.txt" || puts[1].Key != "b.txt" || puts[1].BytesIn != 2 {
		t.Fatalf("PutObject requests = %+v", puts)
	}
	if puts[0].Bucket != "demo" || puts[0].Status != 200 || puts[0].Method != "PUT" || puts[0].RequestID == "" {
		t.Fatalf("record = %+v", puts[0])
	}

	failed, err := s.Requests().List(fakes3.RequestFilter{Status: "error"})
	if err != nil {
		t.Fatal(err)
	}
	if len(failed) != 1 || failed[0].Operation != "GetObject" || failed[0].Status != 404 {
		t.Fatalf("failed requests = %+v", failed)
	}

	if n, err := s.Requests().Count(fakes3.RequestFilter{Bucket: "demo", Limit: 1}); err != nil || n != 4 {
		t.Fatalf("count = %d, %v; want 4", n, err)
	}
	if err := s.Requests().Clear(); err != nil {
		t.Fatal(err)
	}
	if n, _ := s.Requests().Count(fakes3.RequestFilter{}); n != 0 {
		t.Fatalf("count after clear = %d", n)
	}

	if err := s.Requests().SetEnabled(false); err != nil {
		t.Fatal(err)
	}
	put(t, c, "demo", "c.txt", "3")
	cfg, err := s.Requests().Config()
	if err != nil || cfg.Enabled || cfg.Stored != 0 {
		t.Fatalf("config = %+v, %v", cfg, err)
	}
}

func TestSnapshotRestoreReset(t *testing.T) {
	t.Parallel()
	s := fakes3.Start(t)
	c := client(s)
	createBucket(t, c, "demo")
	put(t, c, "demo", "seed.txt", "seed")

	info, err := s.Snapshot("seeded")
	if err != nil {
		t.Fatal(err)
	}
	if info.Name != "seeded" || info.Buckets != 1 || info.Objects != 1 {
		t.Fatalf("snapshot = %+v", info)
	}

	put(t, c, "demo", "extra.txt", "x")
	if _, err := s.Restore("seeded"); err != nil {
		t.Fatal(err)
	}
	if got := keys(t, c, "demo"); len(got) != 1 || got[0] != "seed.txt" {
		t.Fatalf("after restore: %v", got)
	}

	// Reset clears the data but keeps snapshots, so a test can always go back to the seed.
	if err := s.Reset(); err != nil {
		t.Fatal(err)
	}
	if _, err := c.ListObjectsV2(context.Background(), &s3.ListObjectsV2Input{Bucket: aws.String("demo")}); err == nil {
		t.Fatal("bucket survived reset")
	}
	if _, err := s.Restore("seeded"); err != nil {
		t.Fatal(err)
	}
	if got := keys(t, c, "demo"); len(got) != 1 {
		t.Fatalf("after reset and restore: %v", got)
	}

	list, err := s.Snapshots()
	if err != nil || len(list) != 1 {
		t.Fatalf("snapshots = %+v, %v", list, err)
	}
	if err := s.DeleteSnapshot("seeded"); err != nil {
		t.Fatal(err)
	}
	_, err = s.Restore("seeded")
	var e *fakes3.Error
	if !errors.As(err, &e) || e.Status != 404 {
		t.Fatalf("restore of a deleted snapshot: %v", err)
	}

	m, err := s.Memory()
	if err != nil || m.Used <= 0 {
		t.Fatalf("memory = %+v, %v", m, err)
	}
}

func TestOptions(t *testing.T) {
	t.Parallel()
	var logs bytes.Buffer
	s := fakes3.StartWith(t, fakes3.Options{
		AccessKey: "admin",
		SecretKey: "admin-secret",
		Auth:      true,
		Users:     []fakes3.User{{Name: "bob", AccessKey: "bob-key", SecretKey: "bob-secret"}},
		Stderr:    &logs,
	})
	if s.AccessKey != "admin" || s.SecretKey != "admin-secret" {
		t.Fatalf("credentials = %s/%s", s.AccessKey, s.SecretKey)
	}
	createBucket(t, client(s), "admin-bucket")

	// With Auth, a wrong secret is rejected.
	bad := s3.New(s3.Options{
		BaseEndpoint: aws.String(s.Endpoint), Region: s.Region, UsePathStyle: true,
		Credentials: credentials.NewStaticCredentialsProvider("bob-key", "wrong", ""),
	})
	if _, err := bad.ListBuckets(context.Background(), &s3.ListBucketsInput{}); err == nil {
		t.Fatal("wrong secret accepted with Auth")
	}
	bob := s3.New(s3.Options{
		BaseEndpoint: aws.String(s.Endpoint), Region: s.Region, UsePathStyle: true,
		Credentials: credentials.NewStaticCredentialsProvider("bob-key", "bob-secret", ""),
	})
	out, err := bob.ListBuckets(context.Background(), &s3.ListBucketsInput{})
	if err != nil {
		t.Fatal(err)
	}
	if len(out.Buckets) != 0 {
		t.Fatalf("bob sees the admin's buckets: %v", out.Buckets)
	}
}

func TestStartFailureReportsStderr(t *testing.T) {
	t.Parallel()
	_, err := fakes3.New(context.Background(), fakes3.Options{MemoryLimit: "lots"})
	if err == nil {
		t.Fatal("started with an invalid option")
	}
	if !strings.Contains(err.Error(), "lots") {
		t.Fatalf("error does not explain the failure: %v", err)
	}
}

func TestCloseFreesPort(t *testing.T) {
	t.Parallel()
	s, err := fakes3.New(context.Background(), fakes3.Options{})
	if err != nil {
		t.Fatal(err)
	}
	addr := net.JoinHostPort(s.Host, fmt.Sprint(s.Port))
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	if conn, err := net.DialTimeout("tcp", addr, time.Second); err == nil {
		conn.Close()
		t.Fatalf("%s still accepts connections after Close", addr)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
}

// The server must not outlive the test process, whether the test panics (cleanups run) or the
// process exits without running them, as on os.Exit or a `go test -timeout` panic.
func TestNoLeftoverProcess(t *testing.T) {
	for _, mode := range []string{"panic", "goroutine-panic", "exit"} {
		t.Run(mode, func(t *testing.T) {
			t.Parallel()
			file := filepath.Join(t.TempDir(), "endpoint")
			cmd := exec.Command(os.Args[0], "-test.run=^TestHelperProcess$", "-test.v")
			cmd.Env = append(os.Environ(), "FAKES3_TEST_HELPER="+mode, "FAKES3_TEST_HELPER_OUT="+file)
			out, err := cmd.CombinedOutput()
			if err == nil {
				t.Fatalf("helper process succeeded:\n%s", out)
			}
			b, err := os.ReadFile(file)
			if err != nil {
				t.Fatalf("helper did not start a server: %v\n%s", err, out)
			}
			addr := strings.TrimPrefix(string(b), "http://")
			deadline := time.Now().Add(10 * time.Second)
			for {
				conn, err := net.DialTimeout("tcp", addr, time.Second)
				if err != nil {
					return
				}
				conn.Close()
				if time.Now().After(deadline) {
					t.Fatalf("server at %s outlived the test process", addr)
				}
				time.Sleep(50 * time.Millisecond)
			}
		})
	}
}

func TestHelperProcess(t *testing.T) {
	mode := os.Getenv("FAKES3_TEST_HELPER")
	if mode == "" {
		t.Skip("helper for TestNoLeftoverProcess")
	}
	s := fakes3.Start(t)
	if err := os.WriteFile(os.Getenv("FAKES3_TEST_HELPER_OUT"), []byte(s.Endpoint), 0o644); err != nil {
		t.Fatal(err)
	}
	switch mode {
	case "panic":
		panic("test panics")
	case "goroutine-panic":
		done := make(chan struct{})
		go func() {
			defer close(done)
			panic("goroutine panics")
		}()
		<-done
	case "exit":
		os.Exit(3)
	}
}
