# fakes3 for Go

English | [简体中文](README.zh-CN.md)

Start [fakes3](https://github.com/AkaraChen/fakes3), an in-memory S3 server, from your Go tests.
There is no Docker: each server is a small child process that is ready in a few milliseconds, so
every test can have its own.

```bash
go get github.com/AkaraChen/fakes3/bindings/go
```

```go
import (
	"testing"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"

	fakes3 "github.com/AkaraChen/fakes3/bindings/go"
)

func TestUpload(t *testing.T) {
	t.Parallel()
	srv := fakes3.Start(t) // a fresh server on a free port, stopped when the test ends
	client := s3.New(s3.Options{
		BaseEndpoint: aws.String(srv.Endpoint),
		Region:       srv.Region,
		UsePathStyle: true,
		Credentials:  credentials.NewStaticCredentialsProvider(srv.AccessKey, srv.SecretKey, ""),
	})
	// ... call your code with client, then check what's in the bucket
}
```

Every server has its own data, clock and port, so tests running with `t.Parallel()` never see each
other's buckets. The module uses only the standard library; any S3 client works (the example uses
aws-sdk-go-v2). Go 1.24 or later.

## Where the binary comes from

The first match wins:

1. `Options.Binary`
2. the `FAKES3_BINARY` environment variable
3. `fakes3` on `PATH`
4. a download of the fakes3 release this module is pinned to (`fakes3.Version`) from
   [GitHub Releases](https://github.com/AkaraChen/fakes3/releases)

A download happens once per version: the archive for your platform is checked against the SHA-256
checksum published with the release, and the binary is kept in the user cache directory
(`~/.cache/fakes3` on Linux, `~/Library/Caches/fakes3` on macOS, `%LocalAppData%\fakes3` on
Windows). If the checksum doesn't match, `Start` fails the test and nothing is cached.

| Environment variable | Option | Description |
| --- | --- | --- |
| `FAKES3_BINARY` | `Binary` | Path of the fakes3 executable to run |
| `FAKES3_VERSION` | `Version` | Release to download instead of `fakes3.Version`, e.g. `0.1.4` |
| `FAKES3_CACHE_DIR` | `CacheDir` | Where downloaded binaries are kept |
| `FAKES3_DOWNLOAD_URL` | `DownloadURL` | Base URL to download releases from, for a mirror; files are fetched from `<url>/v<version>/<archive>` |

Prebuilt binaries exist for Linux x86_64 and arm64, macOS on Apple silicon and Intel, and Windows
x86_64. Elsewhere, build one with `cargo install fakes3 --locked` and point `FAKES3_BINARY` at it.

In CI, cache the directory to skip the download, or install fakes3 on `PATH` beforehand (for
example with [`setup-fakes3`](https://github.com/AkaraChen/setup-fakes3)).

## Starting a server

| Function | Description |
| --- | --- |
| `Start(t)` | Starts a server with default options. Fails the test if it can't; stops the server in `t.Cleanup` |
| `StartWith(t, opts)` | The same, with `Options` |
| `New(ctx, opts)` | Returns `(*Server, error)` for use outside a test, such as `TestMain`. Call `Close` yourself |

`Options` fields are all optional:

| Field | Default | Description |
| --- | --- | --- |
| `Host` | `"127.0.0.1"` | Address to listen on |
| `AccessKey` / `SecretKey` | `"fakes3"` / `"fakes3secret"` | Credentials of the default account, checked only with `Auth` |
| `Auth` | `false` | Verify signatures. By default any credentials work: a signed request acts as the account its access key belongs to (the default account if none does), and an unsigned request is anonymous, subject to ACLs, bucket policies and Block Public Access |
| `Users` | none | More accounts: `[]fakes3.User{{Name, AccessKey, SecretKey, AccountID, CanonicalID}}`; the ids are optional |
| `BucketDefaults` | `"legacy"` | Settings new buckets start with: `"legacy"`, or `"aws"` for what AWS applies today (ACLs disabled, Block Public Access on, SSE-S3, SSE-C blocked) |
| `MemoryLimit` | `"auto"` | Cap on object data: `"512MiB"`, `"2GiB"`, `"unlimited"`, or `"auto"` (75% of the container's or machine's memory). Writes past it fail with HTTP 507 |
| `Domains` | `["localhost"]` | Base domains for virtual-host addressing (`<bucket>.<domain>`); path-style always works |
| `DisableRequestLog` / `RequestLogCapacity` | `false` / `10000` | Turn the request log off, or change how many requests it keeps |
| `Args` | none | Extra command-line flags for fakes3 |
| `Stderr` | none | Where the server's log goes. Without it, the log only appears in the error when the server fails to start |
| `StartTimeout` | 30s | How long starting may take, not counting a download |

`FAKES3_*` variables that configure the fakes3 server itself (such as `FAKES3_AUTH`) are not passed
on, so a setting in your shell can't change how test servers behave; use `Options`.

## `Server`

| Member | Description |
| --- | --- |
| `Endpoint`, `Host`, `Port` | Where the server listens. `Endpoint` is `http://host:port`, ready for an S3 client with path-style addressing |
| `AccessKey`, `SecretKey`, `Region` | Credentials and region (`us-east-1`) to configure clients with |
| `Close()` | Stops the server; the port is free when it returns. Safe to call more than once |
| `Reset()`, `ResetWith(fakes3.ResetOptions{...})` | Deletes all buckets, objects and uploads, clears the request log and resets the clock. Snapshots are kept unless `Snapshots: true`; `KeepClock` and `KeepRequests` leave those alone |
| `Snapshot(name)`, `Restore(name)`, `Snapshots()`, `DeleteSnapshot(name)` | Save the current state and roll back to it, as often as you like. Object data is shared, not copied. An empty name gets a generated one |
| `Memory()` | Object data held, in bytes, and the limit (0 when unlimited) |
| `Clock()` | The server clock (below) |
| `Requests()` | The request log (below) |

Control methods return an error, a `*fakes3.Error` with the HTTP status and code when the server
refuses (for example `Restore` of a snapshot that doesn't exist). They are not synchronized with S3
requests in flight, so call them while none are pending.

### Virtual clock

Every timestamp the server produces reads this clock, and so does everything that depends on time:
presigned URL expiry, lifecycle rules, Object Lock retention and archive restores.

```go
srv.Clock().FreezeAt(time.Date(2030, 1, 2, 3, 4, 5, 0, time.UTC)) // Freeze() freezes at the current time
srv.Clock().Advance(31 * 24 * time.Hour) // negative moves back
srv.Clock().Set(t)                       // jump, keeping the frozen or running state
srv.Clock().Unfreeze()                   // let time run again
srv.Clock().Reset()                      // back to the host clock
srv.Clock().Now()                        // time.Time; State() also says whether it is frozen or shifted
```

A presigned URL expires when the clock passes its expiry, with the same 403 as S3:

```go
req, _ := s3.NewPresignClient(client).PresignGetObject(ctx, &s3.GetObjectInput{
	Bucket: aws.String("reports"), Key: aws.String("q3.csv"),
}, s3.WithPresignExpires(time.Minute))

srv.Clock().Advance(2 * time.Minute)
resp, _ := http.Get(req.URL) // 403 AccessDenied: Request has expired
```

Moving the clock back doesn't revive a URL that has expired by the host clock, and clients must sign
with the host clock (the AWS SDK does), because signatures more than 15 minutes off are rejected.

### Request log

Assert on what your code actually sent:

```go
puts, err := srv.Requests().List(fakes3.RequestFilter{Bucket: "reports", Operation: "PutObject"})
n, err := srv.Requests().Count(fakes3.RequestFilter{Status: "error"}) // ignores Limit
srv.Requests().Clear()
srv.Requests().SetEnabled(false) // also SetCapacity(n) and Config()
```

Each `fakes3.Request` has `Seq`, `Time`, `Method`, `Path`, `Bucket`, `Key`, `Operation`, `Status`,
`LatencyUs`, `BytesIn`, `BytesOut`, `AccessKey` and `RequestID`, oldest first. Filter fields:
`Bucket`, `KeyPrefix`, `Operation`, `Method`, `Status` (`"404"`, `"4xx"` or `"error"`), `Since`,
`After` (a `Seq`) and `Limit` (keep only the most recent matches).

## One server for a whole package

Starting a server per test is cheap, but you can also share one and put it back in a known state
between tests:

```go
var srv *fakes3.Server

func TestMain(m *testing.M) {
	var err error
	srv, err = fakes3.New(context.Background(), fakes3.Options{})
	if err != nil {
		log.Fatal(err)
	}
	// seed buckets and objects here, then: srv.Snapshot("seeded")
	code := m.Run()
	srv.Close()
	os.Exit(code)
}

func TestSomething(t *testing.T) {
	srv.Restore("seeded") // or srv.Reset()
	// ...
}
```

Tests that share a server can't use `t.Parallel()`.

## Stopping

`Start` stops the server when the test finishes, including when it fails or panics. If the test
binary dies without running cleanups (a panic in another goroutine, `os.Exit`, a `go test -timeout`
or a kill), the server goes with it: on Linux the kernel kills it when its parent exits, on macOS a
small watchdog does, and on Windows it runs in a job object that closes with the test process.

## Versions

`fakes3.Version` is the fakes3 release a module version downloads. Module versions are tagged
`bindings/go/vX.Y.Z` and follow the fakes3 release they pin; `go get
github.com/AkaraChen/fakes3/bindings/go@latest` takes the newest.

## Security

fakes3 is for tests. Signatures aren't verified by default and the control API has no
authentication, so don't expose the server to an untrusted network or store real data in it. It
listens on `127.0.0.1` unless you set `Host`.

## License

MIT
