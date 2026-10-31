// S3 SDK compatibility matrix harness for Go SDKs. See ../SPEC.md.
//
// Usage: matrix -sdk <aws-sdk-go-v2|aws-sdk-go|minio-go|gocloud-blob>
// Configuration comes from the env vars documented in SPEC.md.
package main

import (
	"bytes"
	"context"
	"crypto/md5"
	"crypto/rand"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"hash/crc32"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime/debug"
	"slices"
	"strings"
	"time"

	awshttp "github.com/aws/aws-sdk-go-v2/aws/transport/http"
	"github.com/aws/aws-sdk-go/aws/awserr"
	"github.com/aws/smithy-go"
	"github.com/minio/crc64nvme"
	"github.com/minio/minio-go/v7"
)

var scenarioIDs = []string{
	"create_bucket", "head_bucket", "head_bucket_missing", "list_buckets",
	"put_get_small", "put_get_empty", "put_get_large", "special_keys",
	"head_object_metadata", "get_range", "get_range_invalid", "get_missing_key",
	"get_if_none_match", "get_if_match_fail", "put_if_none_match",
	"delete_object", "delete_missing_object", "delete_objects",
	"list_v2_basic", "list_v2_prefix_delimiter", "list_v2_pagination", "list_v2_start_after", "list_v1",
	"copy_object", "copy_object_replace_metadata",
	"multipart_upload", "multipart_abort", "multipart_list_parts", "multipart_too_small",
	"high_level_upload", "presigned_get", "presigned_put",
	"checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme",
	"content_md5_bad", "bad_credentials", "delete_bucket_not_empty",
	"versioning", "object_tagging", "delete_bucket",
}

// Config holds the env-provided settings.
type Config struct {
	Endpoint, AccessKey, SecretKey, Region, Server, Out string
}

// Env is passed to every scenario.
type Env struct {
	Config
	Bucket string // main bucket, created by create_bucket (or setup fallback)
}

// SDK is implemented by each Go SDK adapter.
type SDK interface {
	Name() string
	Module() string // Go module path used to look up the version
	// Scenarios returns the scenario implementations keyed by ID.
	Scenarios(env *Env) map[string]func(ctx context.Context) error
}

// Skip is returned by a scenario when the SDK lacks the capability.
type Skip string

func (s Skip) Error() string { return string(s) }

type Result struct {
	Scenario string   `json:"scenario"`
	Status   string   `json:"status"`
	Ms       *float64 `json:"ms,omitempty"`
	Error    string   `json:"error,omitempty"`
}

type Report struct {
	SDK     string   `json:"sdk"`
	Lang    string   `json:"lang"`
	Version string   `json:"version"`
	Server  string   `json:"server"`
	Results []Result `json:"results"`
}

func getenv(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

func main() {
	sdkName := flag.String("sdk", "", "sdk to run")
	flag.Parse()
	cfg := Config{
		Endpoint:  getenv("S3_ENDPOINT", "http://127.0.0.1:9100"),
		AccessKey: getenv("S3_ACCESS_KEY", "fakes3"),
		SecretKey: getenv("S3_SECRET_KEY", "fakes3secret"),
		Region:    getenv("S3_REGION", "us-east-1"),
		Server:    getenv("MATRIX_SERVER", "fakes3"),
	}
	cfg.Out = getenv("MATRIX_OUT", filepath.Join("..", "results", cfg.Server))

	sdks := map[string]SDK{
		"aws-sdk-go-v2": &V2SDK{},
		"aws-sdk-go":    &V1SDK{},
		"minio-go":      &MinioSDK{},
		"gocloud-blob":  &GocloudSDK{},
	}
	sdk, ok := sdks[*sdkName]
	if !ok {
		fmt.Fprintf(os.Stderr, "unknown -sdk %q\n", *sdkName)
		os.Exit(2)
	}

	env := &Env{Config: cfg, Bucket: "m-" + strings.ReplaceAll(sdk.Name(), "_", "-") + "-" + randHex(4)}
	scen := sdk.Scenarios(env)
	report := Report{SDK: sdk.Name(), Lang: "go", Version: moduleVersion(sdk.Module()), Server: cfg.Server}

	defer func() {
		// Best-effort cleanup of every bucket this run may have created.
		ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
		defer cancel()
		for _, b := range []string{env.Bucket, env.Bucket + "-d", env.Bucket + "-v"} {
			_ = adminNukeBucket(ctx, cfg, b)
		}
	}()

	for _, id := range scenarioIDs {
		if id == "head_bucket" {
			// Make sure later scenarios have a bucket even if this SDK cannot create one.
			ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
			_ = adminEnsureBucket(ctx, cfg, env.Bucket)
			cancel()
		}
		fn := scen[id]
		var r Result
		if fn == nil {
			r = Result{Scenario: id, Status: "fail", Error: "harness: scenario not implemented"}
		} else {
			r = runOne(id, fn)
		}
		report.Results = append(report.Results, r)
		msg := r.Error
		fmt.Fprintf(os.Stderr, "[%s/%s] %-28s %-4s %s\n", cfg.Server, sdk.Name(), id, r.Status, msg)
	}

	if err := os.MkdirAll(cfg.Out, 0o755); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	data, _ := json.MarshalIndent(report, "", "  ")
	if err := os.WriteFile(filepath.Join(cfg.Out, sdk.Name()+".json"), append(data, '\n'), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

const scenarioTimeout = 60 * time.Second

func runOne(id string, fn func(ctx context.Context) error) Result {
	ctx, cancel := context.WithTimeout(context.Background(), scenarioTimeout)
	defer cancel()
	done := make(chan error, 1)
	start := time.Now()
	go func() {
		defer func() {
			if p := recover(); p != nil {
				done <- fmt.Errorf("panic: %v\n%s", p, debug.Stack())
			}
		}()
		done <- fn(ctx)
	}()
	var err error
	select {
	case err = <-done:
	case <-time.After(scenarioTimeout + 5*time.Second):
		err = fmt.Errorf("timeout: scenario exceeded %s", scenarioTimeout)
	}
	ms := float64(time.Since(start).Microseconds()) / 1000.0
	var skip Skip
	if errors.As(err, &skip) {
		return Result{Scenario: id, Status: "skip", Error: string(skip)}
	}
	if err != nil {
		return Result{Scenario: id, Status: "fail", Ms: &ms, Error: errText(err)}
	}
	return Result{Scenario: id, Status: "pass", Ms: &ms}
}

func moduleVersion(mod string) string {
	if bi, ok := debug.ReadBuildInfo(); ok {
		for _, d := range bi.Deps {
			if d.Path == mod {
				return strings.TrimPrefix(d.Version, "v")
			}
		}
	}
	return "unknown"
}

// ---------- error helpers ----------

// errText renders "<type/code>: <message>" using the first line, max 300 chars.
func errText(err error) string {
	var s string
	var api smithy.APIError
	var aerr awserr.Error
	var merr minio.ErrorResponse
	switch {
	case errors.As(err, &api):
		s = fmt.Sprintf("%s: %s", api.ErrorCode(), api.ErrorMessage())
		if st := statusOf(err); st != 0 {
			s += fmt.Sprintf(" (HTTP %d)", st)
		}
	case errors.As(err, &aerr):
		s = fmt.Sprintf("%s: %s", aerr.Code(), aerr.Message())
		if st := statusOf(err); st != 0 {
			s += fmt.Sprintf(" (HTTP %d)", st)
		}
	case errors.As(err, &merr) && merr.Code != "":
		s = fmt.Sprintf("%s: %s (HTTP %d)", merr.Code, merr.Message, merr.StatusCode)
	default:
		var ae *AssertError
		if errors.As(err, &ae) {
			s = "AssertionError: " + ae.Msg
		} else {
			s = fmt.Sprintf("%T: %v", err, err)
		}
	}
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	if len(s) > 300 {
		s = s[:300]
	}
	return s
}

// statusOf extracts an HTTP status code from any of the SDKs' error types.
func statusOf(err error) int {
	if err == nil {
		return 0
	}
	var re *awshttp.ResponseError
	if errors.As(err, &re) {
		return re.HTTPStatusCode()
	}
	var rf awserr.RequestFailure
	if errors.As(err, &rf) {
		return rf.StatusCode()
	}
	var merr minio.ErrorResponse
	if errors.As(err, &merr) {
		return merr.StatusCode
	}
	return 0
}

// codeOf extracts an S3 error code from any of the SDKs' error types.
func codeOf(err error) string {
	var api smithy.APIError
	if errors.As(err, &api) {
		return api.ErrorCode()
	}
	var aerr awserr.Error
	if errors.As(err, &aerr) {
		return aerr.Code()
	}
	var merr minio.ErrorResponse
	if errors.As(err, &merr) {
		return merr.Code
	}
	return ""
}

type AssertError struct{ Msg string }

func (e *AssertError) Error() string { return e.Msg }

func failf(format string, a ...any) error { return &AssertError{fmt.Sprintf(format, a...)} }

// expectErr passes when err matches one of the given statuses or codes.
func expectErr(what string, err error, statuses []int, codes ...string) error {
	if err == nil {
		return failf("%s: expected error (%v %v), got success", what, statuses, codes)
	}
	if slices.Contains(statuses, statusOf(err)) || slices.Contains(codes, codeOf(err)) {
		return nil
	}
	return fmt.Errorf("%s: expected %v %v, got %w", what, statuses, codes, err)
}

// ---------- assertion helpers ----------

func eqBytes(what string, got, want []byte) error {
	if !bytes.Equal(got, want) {
		return failf("%s: content mismatch (got %d bytes, want %d)", what, len(got), len(want))
	}
	return nil
}

func eqStr(what, got, want string) error {
	if got != want {
		return failf("%s: got %q, want %q", what, got, want)
	}
	return nil
}

func firstErr(errs ...error) error {
	for _, e := range errs {
		if e != nil {
			return e
		}
	}
	return nil
}

func randBytes(n int) []byte {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return b
}

func randHex(n int) string { return hex.EncodeToString(randBytes(n)) }

func md5Hex(b []byte) string { s := md5.Sum(b); return hex.EncodeToString(s[:]) }

func md5B64(b []byte) string { s := md5.Sum(b); return base64.StdEncoding.EncodeToString(s[:]) }

func unquote(etag string) string {
	etag = strings.ReplaceAll(etag, "&#34;", "\"")
	etag = strings.ReplaceAll(etag, "&quot;", "\"")
	return strings.Trim(etag, "\"")
}

// isMultipartETag checks for "<32 hex>-<n>".
func isMultipartETag(etag string, n int) bool {
	etag = unquote(etag)
	parts := strings.SplitN(etag, "-", 2)
	if len(parts) != 2 || len(parts[0]) != 32 || parts[1] != fmt.Sprint(n) {
		return false
	}
	_, err := hex.DecodeString(parts[0])
	return err == nil
}

// checksumB64 computes the base64 S3 checksum value for an algorithm name.
func checksumB64(alg string, data []byte) string {
	var sum []byte
	switch alg {
	case "CRC32":
		sum = binary.BigEndian.AppendUint32(nil, crc32.ChecksumIEEE(data))
	case "CRC32C":
		sum = binary.BigEndian.AppendUint32(nil, crc32.Checksum(data, crc32.MakeTable(crc32.Castagnoli)))
	case "SHA1":
		s := sha1.Sum(data)
		sum = s[:]
	case "SHA256":
		s := sha256.Sum256(data)
		sum = s[:]
	case "CRC64NVME":
		sum = binary.BigEndian.AppendUint64(nil, crc64nvme.Checksum(data))
	}
	return base64.StdEncoding.EncodeToString(sum)
}

var checksumScenarios = map[string]string{
	"checksum_crc32": "CRC32", "checksum_crc32c": "CRC32C", "checksum_sha1": "SHA1",
	"checksum_sha256": "SHA256", "checksum_crc64nvme": "CRC64NVME",
}

// ---------- plain HTTP (for presigned URLs) ----------

func httpDo(ctx context.Context, method, url string, body []byte, hdr http.Header) (int, []byte, error) {
	var rd io.Reader
	if body != nil {
		rd = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(ctx, method, url, rd)
	if err != nil {
		return 0, nil, err
	}
	if body != nil {
		req.ContentLength = int64(len(body))
	}
	for k, v := range hdr {
		req.Header[k] = v
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return 0, nil, err
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(resp.Body)
	return resp.StatusCode, b, err
}

// ---------- shared scenario fixtures ----------

var specialKeys = []string{"sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"}

var metaIn = map[string]string{"color": "Blue", "x-y": "z"}

const (
	mdContentType        = "text/x-matrix; charset=utf-8"
	mdCacheControl       = "max-age=3600, private"
	mdContentDisposition = `attachment; filename="report.txt"`
	mdContentEncoding    = "gzip"
)

func checkMeta(got map[string]string) error {
	for k, v := range metaIn {
		gv, ok := got[k]
		if !ok {
			// Some SDKs canonicalise header-derived keys; spec requires lowercase keys.
			return failf("user metadata key %q missing (got %v)", k, got)
		}
		if gv != v {
			return failf("user metadata %q: got %q want %q", k, gv, v)
		}
	}
	return nil
}

// sortedUnique checks keys are strictly increasing.
func sortedUnique(keys []string) error {
	for i := 1; i < len(keys); i++ {
		if keys[i-1] >= keys[i] {
			return failf("keys not strictly sorted at %d: %q >= %q", i, keys[i-1], keys[i])
		}
	}
	return nil
}

func paginationKeys(prefix string) []string {
	ks := make([]string, 25)
	for i := range ks {
		ks[i] = fmt.Sprintf("%s%02d", prefix, i)
	}
	return ks
}
