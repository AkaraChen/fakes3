# fakes3 for Java

English | [简体中文](README.zh-CN.md)

Start [fakes3](https://github.com/AkaraChen/fakes3), an in-memory S3 server, from your Java tests, with
no Docker. Each server is a small native process that is ready in a few milliseconds, so every test (or
every test class) can have its own, with its own data, clock and port. It comes with a JUnit 5 extension.

The library needs Java 11 or later and depends on nothing but the JDK. JUnit is only needed for the
extension, and you already have it.

## Install

> Not on Maven Central yet. Until it is, run `mvn install` in `bindings/java` to put it in your local
> Maven repository.

Maven:

```xml
<dependency>
  <groupId>io.github.akarachen</groupId>
  <artifactId>fakes3</artifactId>
  <version>0.1.4</version>
  <scope>test</scope>
</dependency>
```

Gradle:

```kotlin
testImplementation("io.github.akarachen:fakes3:0.1.4")
```

## JUnit 5

Add the extension and ask for a `FakeS3` parameter. Each test gets a fresh server, stopped when the test
ends, whether it passed or failed:

```java
import io.github.akarachen.fakes3.FakeS3;
import io.github.akarachen.fakes3.junit.FakeS3Extension;

@ExtendWith(FakeS3Extension.class)
class UploadServiceTest {
  @Test
  void storesTheUpload(FakeS3 s3) {
    S3Client client = S3Client.builder()
        .endpointOverride(URI.create(s3.endpoint()))
        .forcePathStyle(true)
        .region(Region.US_EAST_1)
        .credentialsProvider(StaticCredentialsProvider.create(
            AwsBasicCredentials.create(s3.accessKey(), s3.secretKey())))
        .build();
    client.createBucket(b -> b.bucket("uploads"));

    new UploadService(client).upload("report.pdf", bytes);

    assertEquals(1, s3.requests().count(RequestFilter.create().operation("PutObject")));
  }
}
```

A `@BeforeEach` method that takes a `FakeS3` gets the same server as its test, so you can build your
client there.

**One server per class.** Take the parameter in a `@BeforeAll` method instead. The class's tests then
receive that same server, and you reset it between tests:

```java
@ExtendWith(FakeS3Extension.class)
class CatalogTest {
  static S3Client client;

  @BeforeAll
  static void start(FakeS3 s3) {
    client = /* as above */;
    client.createBucket(b -> b.bucket("catalog"));
    s3.snapshot("empty-catalog");
  }

  @BeforeEach
  void clean(FakeS3 s3) {
    s3.restore("empty-catalog");   // or s3.reset() to drop everything
  }
}
```

**Settings.** Register the extension with a builder:

```java
@RegisterExtension
static FakeS3Extension s3 = new FakeS3Extension(FakeS3.builder().auth(true).bucketDefaults("aws"));
```

## Without JUnit

`FakeS3` is `AutoCloseable`:

```java
try (FakeS3 s3 = FakeS3.start()) {           // 127.0.0.1, a free port
  String endpoint = s3.endpoint();            // "http://127.0.0.1:53124"
  // ...
}

FakeS3 s3 = FakeS3.builder()
    .credentials("test", "test-secret")
    .memoryLimit("512MiB")
    .start();
```

| Builder method | Default | Description |
| --- | --- | --- |
| `host(String)` / `port(int)` | `127.0.0.1` / `0` | Where to listen; port `0` picks a free one |
| `credentials(accessKey, secretKey)` | `fakes3` / `fakes3secret` | The default account's credentials |
| `auth(boolean)` | `false` | Verify signatures. By default any credentials work, and an unsigned request is anonymous |
| `user(name, accessKey, secretKey)` | none | Another account, owning what it creates; repeatable |
| `bucketDefaults(String)` | `"legacy"` | `"aws"` starts new buckets with what AWS applies today (ACLs disabled, Block Public Access on, SSE-S3) |
| `memoryLimit(String)` | `"auto"` | Cap on object data: bytes, `"512MiB"`, `"unlimited"`, or `"auto"` (75% of memory); writes past it get HTTP 507 |
| `domain(String)` | `localhost` | Base domain for virtual-host addressing; repeatable |
| `requestLog(boolean)` / `requestLogCapacity(int)` | `true` / `10000` | Whether to record requests, and how many to keep |
| `arg(String...)` | none | Any other `fakes3` command-line argument |
| `startTimeout(Duration)` | 30 s | How long to wait for the server to be ready |
| `binary(Path)`, `version(String)`, `cacheDir(Path)`, `downloadUrl(URI)` | | Where the executable comes from (below) |

## Test controls

| Method | Description |
| --- | --- |
| `endpoint()`, `host()`, `port()` | Where the server listens. Use path-style addressing (`forcePathStyle(true)`) |
| `accessKey()`, `secretKey()` | The default account's credentials |
| `close()` / `stop()` | Stops the process. Safe to call more than once |
| `reset()`, `reset(ResetOptions)` | Clears all buckets, objects and uploads, and by default the request log and clock. `ResetOptions.create().snapshots(true)` deletes snapshots too |
| `snapshot()`, `snapshot(name)`, `restore(name)`, `snapshots()`, `deleteSnapshot(name)` | Save the current state and roll back to it, as often as you like. Object data is shared, not copied |
| `clock()` | The server clock (below) |
| `requests()` | The request log (below) |
| `memory()` | Object data held, in bytes, and the limit |

### Virtual clock

Every timestamp the server produces reads this clock, and so does everything that depends on time:
presigned URL expiry, lifecycle rules, Object Lock retention and archive restores.

```java
s3.clock().freeze(Instant.parse("2030-01-02T03:04:05Z")); // freeze() alone freezes at the current time
s3.clock().advance(Duration.ofDays(31));   // or advance("1h30m"), advance("-2w")
s3.clock().set(instant);                   // jump, keeping the frozen or running state
s3.clock().unfreeze();                     // let time run again
s3.clock().reset();                        // back to the real time
s3.clock().now();                          // also isFrozen(), isShifted()
```

To test that a presigned URL expires, sign it as usual and move the clock forward:

```java
URL url = presigner.presignGetObject(p -> p
    .signatureDuration(Duration.ofMinutes(1))
    .getObjectRequest(g -> g.bucket("docs").key("report.pdf"))).url();
s3.clock().advance(Duration.ofMinutes(2));
// GET url -> 403 "Request has expired"
```

Signatures themselves are checked against the real time, so sign with the real time and only move the
server clock forward.

### Request log

Assert on what your code actually sent:

```java
List<RecordedRequest> puts = s3.requests().list(RequestFilter.create().operation("PutObject").bucket("demo"));
long errors = s3.requests().count(RequestFilter.create().status("error"));
s3.requests().clear();
s3.requests().setCapacity(50_000);
```

Each `RecordedRequest` has `seq()`, `time()`, `method()`, `path()`, `bucket()`, `key()`, `operation()`,
`status()`, `latencyUs()`, `bytesIn()`, `bytesOut()`, `accessKey()` and `requestId()`. Filters:
`bucket`, `keyPrefix`, `operation`, `method`, `status` (`404`, `"4xx"` or `"error"`), `since`, `after`
(a `seq`) and `limit` (keep only the most recent matches).

Control calls are not synchronized with in-flight S3 requests, so make them between tests. A control
call the server rejects throws `FakeS3Exception`, with the server's error code in `code()` (for example
`NotFound` for an unknown snapshot). After `close()`, they throw `IllegalStateException`.

## Where the executable comes from

The first match wins:

1. `binary(Path)` on the builder, the `fakes3.binary` system property, or the `FAKES3_BINARY`
   environment variable.
2. `fakes3` on `PATH` (`fakes3.exe` on Windows), used as it is.
3. A download of the matching [release](https://github.com/AkaraChen/fakes3/releases) archive for your
   system (Linux x86_64 and aarch64, macOS Apple silicon and Intel, Windows x86_64). The archive is
   checked against the release's `.sha256` file and nothing is installed if they differ. The binary is
   cached, so this happens once per version and machine.

| Setting | System property | Environment variable | Default |
| --- | --- | --- | --- |
| Release to download | `fakes3.version` | `FAKES3_VERSION` | The version this library was built for (0.1.4) |
| Cache directory | `fakes3.cacheDir` | `FAKES3_CACHE_DIR` | `~/.cache/fakes3` (or `$XDG_CACHE_HOME`), `~/Library/Caches/fakes3` on macOS, `%LOCALAPPDATA%\fakes3\cache` on Windows |
| Download URL | `fakes3.downloadUrl` | `FAKES3_DOWNLOAD_URL` | `https://github.com/AkaraChen/fakes3/releases/download`; a mirror must use the same `v<version>/<archive>` layout |

The download goes through the JVM's default proxy settings (`-Dhttps.proxyHost` and friends). For
offline CI, either cache the directory above or put `fakes3` on `PATH`, for example with
`cargo install fakes3 --locked` or the [setup-fakes3](https://github.com/AkaraChen/setup-fakes3) action.

## Cleanup

`close()` stops the server, and the JUnit extension calls it at the end of each test or class. Servers
still running when the JVM exits normally, by `System.exit`, by an uncaught exception or on Ctrl-C /
SIGTERM are stopped by a shutdown hook. A JVM that is killed outright (`kill -9`, or a forced kill from an
IDE or on Windows) can't run that hook, and its servers keep running until you stop them.

## Security

fakes3 is for tests. Signatures aren't verified by default and the control plane has no authentication,
so don't expose the server to an untrusted network or store real data in it. It listens on `127.0.0.1`
unless you set `host`.

## License

MIT
