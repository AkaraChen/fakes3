# fakes3 的 Java 库

[English](README.md) | 简体中文

在 Java 测试里启动 [fakes3](https://github.com/AkaraChen/fakes3)（内存版的 S3 服务），不需要 Docker。每个服务是一个很小的本地进程，
几毫秒就绪，所以可以每个测试（或每个测试类）各起一个，数据、时钟和端口互不影响。附带 JUnit 5 扩展。

需要 Java 11 及以上，除 JDK 外没有任何依赖。只有 JUnit 扩展用到 JUnit，而你的项目里本来就有。

## 安装

> 还没有发布到 Maven Central。在那之前，在 `bindings/java` 里运行 `mvn install`，把它装进本地 Maven 仓库。

Maven：

```xml
<dependency>
  <groupId>io.github.akarachen</groupId>
  <artifactId>fakes3</artifactId>
  <version>0.1.4</version>
  <scope>test</scope>
</dependency>
```

Gradle：

```kotlin
testImplementation("io.github.akarachen:fakes3:0.1.4")
```

## JUnit 5

加上扩展，在参数里要一个 `FakeS3`。每个测试拿到一个新的服务，测试结束时（不论通过还是失败）自动停止：

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

`@BeforeEach` 方法如果也要 `FakeS3` 参数，拿到的是和测试同一个服务，可以在那里创建客户端。

**每个测试类一个服务。** 改为在 `@BeforeAll` 方法里要这个参数。这个类的测试都会拿到同一个服务，测试之间自己重置：

```java
@ExtendWith(FakeS3Extension.class)
class CatalogTest {
  static S3Client client;

  @BeforeAll
  static void start(FakeS3 s3) {
    client = /* 同上 */;
    client.createBucket(b -> b.bucket("catalog"));
    s3.snapshot("empty-catalog");
  }

  @BeforeEach
  void clean(FakeS3 s3) {
    s3.restore("empty-catalog");   // 或者 s3.reset() 清空所有数据
  }
}
```

**配置。** 用 builder 注册扩展：

```java
@RegisterExtension
static FakeS3Extension s3 = new FakeS3Extension(FakeS3.builder().auth(true).bucketDefaults("aws"));
```

## 不用 JUnit

`FakeS3` 实现了 `AutoCloseable`：

```java
try (FakeS3 s3 = FakeS3.start()) {           // 127.0.0.1，随机空闲端口
  String endpoint = s3.endpoint();            // "http://127.0.0.1:53124"
  // ...
}

FakeS3 s3 = FakeS3.builder()
    .credentials("test", "test-secret")
    .memoryLimit("512MiB")
    .start();
```

| Builder 方法 | 默认值 | 说明 |
| --- | --- | --- |
| `host(String)` / `port(int)` | `127.0.0.1` / `0` | 监听地址；端口 `0` 表示随机选一个空闲端口 |
| `credentials(accessKey, secretKey)` | `fakes3` / `fakes3secret` | 默认账户的凭证 |
| `auth(boolean)` | `false` | 校验签名。默认任何凭证都能用，不带签名的请求是匿名请求 |
| `user(name, accessKey, secretKey)` | 无 | 增加一个账户，它创建的东西归它所有；可重复 |
| `bucketDefaults(String)` | `"legacy"` | `"aws"` 让新建的 bucket 采用 AWS 现在的默认值（禁用 ACL、开启 Block Public Access、SSE-S3） |
| `memoryLimit(String)` | `"auto"` | 对象数据上限：字节数、`"512MiB"`、`"unlimited"` 或 `"auto"`（内存的 75%）；超出的写入返回 HTTP 507 |
| `domain(String)` | `localhost` | virtual-host 寻址的基础域名；可重复 |
| `requestLog(boolean)` / `requestLogCapacity(int)` | `true` / `10000` | 是否记录请求，以及保留多少条 |
| `arg(String...)` | 无 | 其他 `fakes3` 命令行参数 |
| `startTimeout(Duration)` | 30 秒 | 等待服务就绪的时间 |
| `binary(Path)`、`version(String)`、`cacheDir(Path)`、`downloadUrl(URI)` | | 可执行文件从哪里来（见下文） |

## 测试控制

| 方法 | 说明 |
| --- | --- |
| `endpoint()`、`host()`、`port()` | 服务地址。客户端请用 path-style 寻址（`forcePathStyle(true)`） |
| `accessKey()`、`secretKey()` | 默认账户的凭证 |
| `close()` / `stop()` | 停止进程，可以重复调用 |
| `reset()`、`reset(ResetOptions)` | 清空所有 bucket、对象和分段上传，默认同时清空请求日志、恢复时钟。`ResetOptions.create().snapshots(true)` 连快照一起删除 |
| `snapshot()`、`snapshot(name)`、`restore(name)`、`snapshots()`、`deleteSnapshot(name)` | 保存当前状态并随时回滚，可以反复 restore。对象数据是共享的，不复制 |
| `clock()` | 服务器时钟（见下文） |
| `requests()` | 请求日志（见下文） |
| `memory()` | 持有的对象数据字节数和上限 |

### 虚拟时钟

服务器产生的所有时间戳都读这个时钟，依赖时间的功能也是：预签名 URL 过期、Lifecycle 规则、Object Lock 保留期、归档对象的恢复。

```java
s3.clock().freeze(Instant.parse("2030-01-02T03:04:05Z")); // 不带参数的 freeze() 冻结在当前时间
s3.clock().advance(Duration.ofDays(31));   // 或者 advance("1h30m")、advance("-2w")
s3.clock().set(instant);                   // 跳到某个时间，保持冻结或运行的状态
s3.clock().unfreeze();                     // 让时间继续走
s3.clock().reset();                        // 回到真实时间
s3.clock().now();                          // 还有 isFrozen()、isShifted()
```

测试预签名 URL 过期：照常签名，然后把时钟往前拨：

```java
URL url = presigner.presignGetObject(p -> p
    .signatureDuration(Duration.ofMinutes(1))
    .getObjectRequest(g -> g.bucket("docs").key("report.pdf"))).url();
s3.clock().advance(Duration.ofMinutes(2));
// GET url -> 403 "Request has expired"
```

签名本身仍按真实时间校验，所以请用真实时间签名，只把服务器时钟往前拨。

### 请求日志

断言代码实际发了哪些请求：

```java
List<RecordedRequest> puts = s3.requests().list(RequestFilter.create().operation("PutObject").bucket("demo"));
long errors = s3.requests().count(RequestFilter.create().status("error"));
s3.requests().clear();
s3.requests().setCapacity(50_000);
```

每条 `RecordedRequest` 有 `seq()`、`time()`、`method()`、`path()`、`bucket()`、`key()`、`operation()`、`status()`、
`latencyUs()`、`bytesIn()`、`bytesOut()`、`accessKey()` 和 `requestId()`。过滤条件：`bucket`、`keyPrefix`、`operation`、
`method`、`status`（`404`、`"4xx"` 或 `"error"`）、`since`、`after`（一个 `seq`）和 `limit`（只保留最近的 N 条匹配）。

控制调用和进行中的 S3 请求没有同步，请在测试之间调用。服务器拒绝的控制调用会抛出 `FakeS3Exception`，`code()` 是服务器的错误码
（例如快照不存在时是 `NotFound`）。`close()` 之后再调用会抛出 `IllegalStateException`。

## 可执行文件从哪里来

按顺序取第一个找到的：

1. builder 的 `binary(Path)`、系统属性 `fakes3.binary` 或环境变量 `FAKES3_BINARY`。
2. `PATH` 上的 `fakes3`（Windows 上是 `fakes3.exe`），原样使用。
3. 下载与当前系统匹配的 [release](https://github.com/AkaraChen/fakes3/releases) 压缩包（Linux x86_64 和 aarch64、
   Apple silicon 和 Intel 的 macOS、Windows x86_64）。压缩包会和 release 里的 `.sha256` 文件比对，不一致就什么都不安装。
   二进制会缓存下来，每台机器每个版本只下载一次。

| 设置 | 系统属性 | 环境变量 | 默认值 |
| --- | --- | --- | --- |
| 下载哪个版本 | `fakes3.version` | `FAKES3_VERSION` | 构建这个库时对应的版本（0.1.4） |
| 缓存目录 | `fakes3.cacheDir` | `FAKES3_CACHE_DIR` | `~/.cache/fakes3`（或 `$XDG_CACHE_HOME`），macOS 上是 `~/Library/Caches/fakes3`，Windows 上是 `%LOCALAPPDATA%\fakes3\cache` |
| 下载地址 | `fakes3.downloadUrl` | `FAKES3_DOWNLOAD_URL` | `https://github.com/AkaraChen/fakes3/releases/download`；镜像要保持 `v<版本>/<压缩包>` 的路径结构 |

下载走 JVM 默认的代理设置（`-Dhttps.proxyHost` 等）。离线的 CI 可以缓存上面的目录，或者把 `fakes3` 放到 `PATH` 上，例如用
`cargo install fakes3 --locked` 或 [setup-fakes3](https://github.com/AkaraChen/setup-fakes3) action。

## 清理

`close()` 停止服务，JUnit 扩展在每个测试或测试类结束时调用它。JVM 正常退出、`System.exit`、未捕获的异常、Ctrl-C 或 SIGTERM 时，
还在运行的服务由 shutdown hook 停止。被直接杀掉的 JVM（`kill -9`、IDE 或 Windows 上的强制结束）来不及运行这个 hook，
它启动的服务会一直运行，需要手动停止。

## 安全提示

fakes3 只用于测试。默认不校验签名，控制面也没有认证，所以不要把服务暴露在不可信的网络上，也不要存放真实数据。除非设置了 `host`，
它只监听 `127.0.0.1`。

## 许可证

MIT
