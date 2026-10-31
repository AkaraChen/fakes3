# fakes3

一个专门给测试用的 S3 服务：全内存、无持久化，启动只要几毫秒，请求延迟在百微秒级。
协议层基于 [s3s](https://github.com/Nugine/s3s)，用 Rust 编写。

它放弃了测试用不到的东西（持久化、fsync、多节点、静态加密），换来启动速度和请求延迟；
但在客户端看得到的地方，尽量和 AWS S3 保持一致：签名、错误码、ETag 规则、版本控制、条件请求、校验和等等。

## 快速开始

用 Docker（镜像说明见[下面](#docker-镜像ghcr)）：

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3
```

在测试代码里直接内嵌：[Rust](#rust-库模式)、[Node.js](#nodejs-绑定)。

或者直接编译运行：

```bash
cargo build --release
./target/release/fakes3 --port 0      # 0 表示随机选一个空闲端口，实际地址会打印到 stdout
```

客户端按 path-style 寻址连接，默认凭证是 `fakes3` / `fakes3secret`：

```python
import boto3
from botocore.config import Config

s3 = boto3.client(
    "s3",
    endpoint_url="http://127.0.0.1:9000",
    aws_access_key_id="fakes3",
    aws_secret_access_key="fakes3secret",
    region_name="us-east-1",
    config=Config(s3={"addressing_style": "path"}),
)
s3.create_bucket(Bucket="demo")
s3.put_object(Bucket="demo", Key="hello.txt", Body=b"hi")
```

## Docker 镜像（GHCR）

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3:latest
```

- 平台：`linux/amd64` 和 `linux/arm64`，同一个 tag 自动选择；镜像基于 Alpine，里面只有一个静态链接的 musl 二进制，压缩后约 7 MB。
- tag：`latest`、完整版本（`0.1.0`）、`主版本.次版本`（`0.1`），以及提交的短 sha（`sha-abc1234`）。
- 镜像里默认监听 `0.0.0.0:9000`，配置项见下面的[配置](#配置)，用环境变量（`FAKES3_*`）或命令行参数都可以。
  启动完成后在 stdout 打印一行 `http://0.0.0.0:9000`，可以作为就绪信号。
- 不想用 Docker 的话，每个 [release](https://github.com/AkaraChen/fakes3/releases) 里也有静态链接的
  `fakes3-x86_64-unknown-linux-musl.tar.gz` 和 `fakes3-aarch64-unknown-linux-musl.tar.gz`（附 sha256）。
- 自己构建：`docker build -t fakes3 .`

发布由 [release workflow](.github/workflows/release.yml) 完成：推送 `v*` tag 时构建两个架构（各用原生 runner）、
推送镜像并把二进制挂到 GitHub release；手动触发时 `publish` 默认是 `false`，只构建和冒烟测试，不推送任何东西。

### 在 Testcontainers 里使用

不需要专门的 Testcontainers 模块：通用容器（generic container）加一个等待策略就够了。
暴露 9000 端口，等 stdout 里出现 `http://`（或者等端口监听），然后用映射出来的主机和端口作为 endpoint。
客户端用 path-style 寻址，默认凭证 `fakes3` / `fakes3secret`。

Python（[testcontainers-python](https://github.com/testcontainers/testcontainers-python)）：

```python
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs

with DockerContainer("ghcr.io/akarachen/fakes3:latest").with_exposed_ports(9000) as c:
    wait_for_logs(c, "http://")
    endpoint = f"http://{c.get_container_host_ip()}:{c.get_exposed_port(9000)}"
    # boto3.client("s3", endpoint_url=endpoint, ...)
```

Node.js / TypeScript（[testcontainers-node](https://github.com/testcontainers/testcontainers-node)）：

```ts
import { GenericContainer, Wait } from "testcontainers";

const container = await new GenericContainer("ghcr.io/akarachen/fakes3:latest")
  .withExposedPorts(9000)
  .withWaitStrategy(Wait.forLogMessage("http://"))
  .start();
const endpoint = `http://${container.getHost()}:${container.getMappedPort(9000)}`;
// ...
await container.stop();
```

Java（[Testcontainers for Java](https://java.testcontainers.org/)）：

```java
GenericContainer<?> s3 = new GenericContainer<>("ghcr.io/akarachen/fakes3:latest")
    .withExposedPorts(9000)
    .waitingFor(Wait.forLogMessage(".*http://.*\\n", 1));
s3.start();
String endpoint = "http://" + s3.getHost() + ":" + s3.getMappedPort(9000);
```

Go（[testcontainers-go](https://golang.testcontainers.org/)）：

```go
c, err := testcontainers.GenericContainer(ctx, testcontainers.GenericContainerRequest{
    ContainerRequest: testcontainers.ContainerRequest{
        Image:        "ghcr.io/akarachen/fakes3:latest",
        ExposedPorts: []string{"9000/tcp"},
        WaitingFor:   wait.ForLog("http://"),
    },
    Started: true,
})
host, _ := c.Host(ctx)
port, _ := c.MappedPort(ctx, "9000")
endpoint := fmt.Sprintf("http://%s:%s", host, port.Port())
```

其他语言（.NET、Rust、Ruby……）同理。容器启动只要几毫秒，所以可以每个测试类甚至每个测试起一个新的，不必共享。

## Rust 库模式

`fakes3` 同时是一个库：测试里直接在进程内启动服务，不需要 Docker，也不需要子进程。
目前没有发布到 crates.io，用 git 依赖：

```toml
[dev-dependencies]
fakes3 = { git = "https://github.com/AkaraChen/fakes3" }
```

异步测试：

```rust
let server = fakes3::Server::start(fakes3::Config::default()).await?;
let endpoint = server.endpoint(); // "http://127.0.0.1:<随机端口>"
// 用 endpoint 和 Config 里的凭证创建 S3 客户端，开启 path-style
server.shutdown().await;
```

同步测试用 `Server::start_blocking`：服务跑在自己的后台 Tokio runtime 上，句柄 drop 时自动停止
（可以在任何地方 drop，包括 async 代码里）：

```rust
let server = fakes3::Server::start_blocking(fakes3::Config::default())?;
let endpoint = server.endpoint();
// server 离开作用域时停止
```

- `Config::default()` 监听 `127.0.0.1` 的随机端口，所以并行的测试不会冲突；每个 `Server` 有独立的数据和虚拟时钟。
- `Server::addr()` / `endpoint()` 返回实际绑定的地址；`shutdown()` 关闭监听并等待，端口随即可以复用。
  已建立的连接会继续服务到客户端断开。
- 完整例子：[examples/aws_sdk.rs](examples/aws_sdk.rs)（`cargo run --example aws_sdk`）、
  [examples/blocking.rs](examples/blocking.rs)，用 `aws-sdk-s3` 的集成测试在 [tests/aws_sdk.rs](tests/aws_sdk.rs)。
- 限制：只能在 Linux 上编译（大对象用 `memfd`）；编译时会从源码构建 OpenSSL，需要 `perl` 和 `make`。

## Node.js 绑定

[bindings/node/](bindings/node/) 是一个 [napi-rs](https://napi.rs) 插件，在 Node 进程里直接运行服务（不起子进程、不需要 Docker），
包名 `fakes3`（**尚未发布到 npm**，名字见 [bindings/node/package.json](bindings/node/package.json)）。

```ts
import { start } from "fakes3";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 默认监听 127.0.0.1 的随机端口
const s3 = new S3Client({
  endpoint: server.endpoint,
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
});
await s3.send(new CreateBucketCommand({ Bucket: "demo" }));
// ...
await server.stop();
```

`start(options?)` 返回 `Promise<FakeS3Server>`，所有选项都可省略：

| 选项 | 默认值 | 说明 |
|---|---|---|
| `host` | `127.0.0.1` | 监听地址 |
| `port` | `0` | 监听端口，`0` 表示随机端口 |
| `accessKey` / `secretKey` | `fakes3` / `fakes3secret` | 访问凭证 |
| `noAuth` | `false` | 不校验签名：用任意 access key 和 secret 签名的请求都接受，未签名的也接受（见下面“`--no-auth`”） |
| `inlineThreshold` | `1048576` | 达到这个大小的对象存进 memfd |
| `requestLog` / `requestLogCapacity` | `true` / `10000` | 是否记录 S3 请求（见下面的控制面），以及最多保留多少条 |
| `domains` | 无 | 虚拟主机寻址的基础域名（`<bucket>.<domain>`），path-style 始终可用 |
| `memoryLimit` | `"auto"` | 对象数据占用内存的上限：字节数，或 `"512MiB"`、`"2GiB"`、`"unlimited"`、`"auto"`；超出时写入返回 HTTP 507 |

`FakeS3Server` 有只读属性 `endpoint`、`host`、`port`、`accessKey`、`secretKey`，以及方法 `memory(): { used, limit }`（当前用量和上限，字节；无上限时没有 `limit` 字段），`stop(): Promise<void>`（可重复调用，调用后端口立即释放）。
类型声明由 `napi build` 生成。

**测试控制面**：[控制面](docs/control-plane.md)的操作也有对应的方法，直接在进程内调用，不走 HTTP。时间一律用 JS 的 `Date`。

```ts
server.reset();                         // 清空所有数据、请求日志和时钟；快照保留（reset({ snapshots: true }) 一并删除）
server.snapshot("seeded");              // 保存当前状态 → { name, created, buckets, objects, uploads }
server.restore("seeded");               // 回滚到快照（快照不会被消耗）；snapshots() 列出，deleteSnapshot(name) 删除
server.memory();                        // { used, limit }

server.clock.freeze(new Date("2030-01-02T03:04:05Z"));   // freeze() 不带参数则冻结在当前时间
server.clock.advance("31d");            // 数字按毫秒，字符串如 "1h30m"、"-2w"；还有 set(date)、unfreeze()、reset()
server.clock.now();                     // Date；isFrozen()、isShifted()

server.requests.list({ operation: "PutObject", status: "error", limit: 10 }); // 过滤条件都可省略
server.requests.count({ bucket: "demo" });   // 匹配的条数（忽略 limit）
server.requests.clear();                // configure({ enabled, capacity }) 开关日志、改容量
```

`requests.list()` 返回按到达顺序排列的记录：`{ seq, time, method, path, bucket, key, operation, status, latencyUs, bytesIn, bytesOut }`。
过滤条件：`bucket`、`keyPrefix`、`operation`、`method`、`status`（`404`、`"4xx"` 或 `"error"`）、`since`、`after`（`seq`）、`limit`（只保留最近的几条）。
和控制面一样，这些调用没有和进行中的请求同步，请在测试之间、没有未完成请求时使用。服务停止后调用会抛出 `server is stopped`。

预编译产物覆盖 Linux 的 x64 / arm64，glibc 和 musl（Alpine）都有，通过 `optionalDependencies` 里的
`fakes3-linux-x64-gnu` 等平台包分发；macOS 和 Windows 暂不支持（服务端依赖 Linux 的 `memfd`）。
自己编译：

```bash
cd bindings/node
npm ci && npm run build   # 需要 Rust 工具链、perl 和 make
npm test                  # 用 @aws-sdk/client-s3 跑一遍
```

CI 在 [node workflow](.github/workflows/node.yml) 里构建这四个目标并各自跑一遍测试；
发布到 npm 只在推送 `v*` tag 或手动触发且 `publish: true` 时发生，其余情况只做 `npm publish --dry-run`。

## 配置

| 参数 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| `--host` | `FAKES3_HOST` | `127.0.0.1`（镜像里是 `0.0.0.0`） | 监听地址 |
| `--port` | `FAKES3_PORT` | `9000` | 监听端口，`0` 表示随机端口 |
| `--access-key` | `FAKES3_ACCESS_KEY` | `fakes3` | 访问凭证 |
| `--secret-key` | `FAKES3_SECRET_KEY` | `fakes3secret` | 访问凭证 |
| `--no-auth` | | 关 | 不校验签名：任意 access key / secret 签的名都接受，未签名的也接受，且不做授权（见下文） |
| `--domain` | `FAKES3_DOMAIN` | `localhost` | virtual-host 寻址的基础域名，可重复（或逗号分隔）；`<bucket>.<domain>` 选择 bucket，其它主机名仍按 path-style 处理；传空字符串关闭 |
| `--inline-threshold` | | `1048576` | 达到这个大小的对象或 part 存进 memfd，小于它的放在堆上 |
| `--memory-limit` | `FAKES3_MEMORY_LIMIT` | `auto` | 对象数据占用内存的上限，见下文。可以写 `2GiB`、`512MiB`、`unlimited`、`auto` |

**`--no-auth`**（库里是 `Config { auth: false, .. }`，Node 里是 `noAuth`）：LocalStack / moto 式的“任意凭证都可以”。
SDK 用什么 access key 和 secret 签名都行，签名本身不校验。覆盖 SigV4 的 `Authorization` 头和 presigned URL、SigV2 的头和 presigned URL，
以及流式上传（`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`、`STREAMING-UNSIGNED-PAYLOAD-TRAILER`、`STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`）；
未签名的请求照常可用。几点说明：

- **完整性校验仍然生效**：aws-chunked 的分块照常解码，trailer 里的校验和（`x-amz-checksum-*`）仍会和数据比对，不一致返回 400 `BadDigest`；
  `Content-MD5`、header 里的校验和、`x-amz-decoded-content-length` 也一样。不再检查的是签名本身以及由它派生的东西：
  时钟偏差、`x-amz-content-sha256` 里的负载哈希、signed headers 列表。
- **不做授权**：ACL、Bucket Policy、Block Public Access 都不求值，已签名和匿名的请求一视同仁，都有 owner 的权限。
  需要测访问控制时用默认的认证模式。
- **presigned URL 的过期**：只在使用虚拟时钟（`/_fakes3/clock`）时判断，真实时钟下不检查。
- 请求日志的 `access_key` 字段记录请求用的 access key（认证模式下它永远是配置的那一个，所以不记录）。
- 带签名的 POST 表单上传（表单里有 `x-amz-signature` / `signature` 字段）暂不支持，仍返回 501；不带签名的表单可用。

**内存上限**：fakes3 统计所有对象数据实际占用的内存（堆上的小对象、memfd 里的大对象、尚未 Complete 的分段上传的 part），
超出上限的写入会被拒绝，而不是让进程被 OOM killer 杀掉。

- 默认值 `auto` 是容器 cgroup 内存限制和物理内存中较小者的 75%（读不到时不限制）。memfd 的页面算在 cgroup 的内存里，
  不设上限的话，容器超限时进程会被直接杀掉，没有任何报错；留 25% 给元数据、连接缓冲区和测试进程自己。
  机器上跑多个 fakes3 或其他大内存进程时，每个实例各自按这个比例计算，需要的话用 `--memory-limit` 显式指定。
  设成 `unlimited`（或 `0`）取消限制。作为库使用时对应 `Config::memory_limit`（`MemoryLimit::Auto` / `Unlimited` / `Bytes(n)`），
  当前用量和上限可以用 `Server::memory()` 读取；用量超过 90% 时会打印一条 warning（默认日志级别就是 `warn`）。
- 统计的是数据 buffer 本身，不含元数据、key 和分配器开销。同一个 buffer 被多个对象、版本、CopyObject、UploadPartCopy、
  CompleteMultipartUpload 共享时只算一次，最后一个引用释放时才归还；覆盖、删除、删除版本、Abort 都会释放。
  因此删掉一个仍被副本引用的对象，内存不会下降。
- 写入时先按 `Content-Length` 一次性预留，放不下就在读取请求体**之前**拒绝（带 `Expect: 100-continue` 的客户端根本不会发送请求体）；
  没有长度的请求体（chunked）边读边计，越线立刻失败并释放已缓冲的部分。拒绝前会把剩余请求体读掉丢弃，
  这样客户端收到的是明确的错误响应，而不是连接被重置。
- 覆盖同一个 key 时，新对象写完前旧对象仍可读，所以瞬时需要同时容纳新旧两份。
- 超限时返回 **HTTP 507 Insufficient Storage**，错误码 `XFakeS3MemoryLimitExceeded`，消息写明已用量、上限和调大的方法。
  不用 503 SlowDown 是因为 SDK 会对它退避重试，只会把失败拖慢；不用 400 EntityTooLarge 是因为它表示单个对象太大。
  各主流 SDK 的默认重试策略都不会重试 507（MinIO 的磁盘满也是 507）。

## 支持的功能

- Bucket：创建、删除、HeadBucket、ListBuckets、GetBucketLocation、Bucket 标签
- 对象：PUT、GET、HEAD、DELETE、DeleteObjects、CopyObject（支持替换元数据和标签）
- 列举：ListObjectsV2、ListObjects（v1）、ListObjectVersions、ListMultipartUploads，支持 prefix、delimiter、分页、`encoding-type=url`
- 分段上传：Create、UploadPart、UploadPartCopy、ListParts、Complete、Abort；GET/HEAD 支持 `?partNumber=`
- 版本控制：Enabled / Suspended、删除标记、null version、按 VersionId 读取和删除、MFA Delete 头校验
- Object Lock：保留期（GOVERNANCE / COMPLIANCE）、Legal Hold、bucket 默认保留期、`x-amz-bypass-governance-retention`，到期时间按服务端时钟判断
- 加密：SSE-C（校验算法和 key MD5，GET/HEAD/Copy 必须带同一个 key）、SSE-S3 / SSE-KMS 头回显、bucket 默认加密；数据本身不加密
- 对象属性：GetObjectAttributes、storage class 和 `x-amz-website-redirect-location` 的写入与回显
- 条件请求：If-Match、If-None-Match（包括条件写入）、If-Modified-Since、If-Unmodified-Since、CopySource 的条件头
- 校验和：CRC32、CRC32C、CRC64NVME、SHA1、SHA256 等，支持 header 和 aws-chunked trailer 两种方式；
  分段上传支持 COMPOSITE 和 FULL_OBJECT；没指定算法时默认计算 CRC64NVME，和现在的 AWS S3 一致
- 认证：SigV4（包括 presigned URL 和带 trailer 的流式上传）、SigV2
- 对象标签、Range 读取、Content-MD5 校验
- 访问控制：Bucket/Object ACL（canned、grant 头、请求体）、OwnershipControls（BucketOwnerEnforced 时 ACL 被禁用）、
  PublicAccessBlock、Bucket Policy（含 Condition，显式 Deny 对已签名请求也生效）；未签名的匿名请求在 policy 或公开 ACL 允许时放行，
  `--no-auth` 时不做授权（见“`--no-auth`”）
- CORS：预检 `OPTIONS` 和实际响应上的 CORS 头
- Lifecycle：过期（含版本控制下的删除标记、非当前版本、`NewerNoncurrentVersions`）和清理未完成的分段上传，按服务端虚拟时钟在 UTC 日期变化后的第一次访问时执行；
  没有规则的 bucket 零开销
- Website、Notification 配置的存取（不提供网站端点，不投递事件）；POST policy 表单上传

还没做的功能见 [TODO.md](TODO.md)，主要是 Website 端点，以及测试控制面里的故障注入。

## 测试控制面

同一个端口上的 `/_fakes3/` 前缀是一组 JSON HTTP API，`fakes3 ctl` 是它的命令行客户端，Rust 进程内使用时是 `fakes3::Server` 上的方法：

```bash
fakes3 ctl reset                              # 清空所有 bucket、对象和上传，同时清空请求日志和时钟
fakes3 ctl snapshot create seeded             # 保存当前状态（不复制对象数据）
fakes3 ctl snapshot restore seeded            # 回滚
fakes3 ctl clock advance 31d                  # 推进服务器时间（也可以 freeze / set），预签名 URL 会跟着过期
fakes3 ctl requests list --op PutObject       # 请求日志：按 bucket、操作、状态码等过滤，用来断言调用次数和顺序
```

端点、命令、各操作的语义和限制见 [docs/control-plane.md](docs/control-plane.md)。控制面没有认证，只适合在测试环境里使用。

## 兼容性

[matrix/](matrix/) 里有一个 SDK 兼容矩阵：**8 种语言、24 个 SDK，每个跑 43 个场景**，同时对 fakes3 和 5 个常见实现跑一遍。
场景定义见 [matrix/SPEC.md](matrix/SPEC.md)，完整结果见 [matrix/MATRIX.md](matrix/MATRIX.md)。

| 实现 | 通过率 |
|---|---:|
| **fakes3** | **99.1%** |
| MinIO | 96.4% |
| LocalStack | 91.4% |
| moto | 87.9% |
| S3Mock | 85.4% |
| JuiceFS gateway | 69.3% |

fakes3 剩下的 8 个失败全部是 SDK 自身的 bug（minio-dotnet 有 6 个，minio-java 和 obstore 处理不了 `a//b` 这种 key），换哪个服务都会失败。

覆盖的 SDK：
- Python：boto3、minio-py、s3fs、obstore
- JavaScript：AWS SDK v3、AWS SDK v2、minio-js、Bun.S3Client
- Go：aws-sdk-go-v2、aws-sdk-go、minio-go、gocloud.dev/blob
- Rust：aws-sdk-s3、object_store、opendal、rust-s3
- Java：AWS SDK v2、AWS SDK v2 CRT、AWS SDK v1、minio-java
- .NET：AWSSDK.S3、Minio
- Ruby：aws-sdk-s3
- PHP：aws-sdk-php

## 性能

所有实现都跑在 Docker 里，使用 host 网络，数据目录放在 tmpfs 上，每项测量都用一个新启动的容器。
下面的数据来自 GitHub Actions 上的一次运行（4 核托管 runner），完整报告在
[这次运行的 job summary](https://github.com/AkaraChen/fakes3/actions/runs/41174196107) 里。

**启动和空闲内存**

| | fakes3 | MinIO | JuiceFS | moto | S3Mock | LocalStack |
|---|---:|---:|---:|---:|---:|---:|
| 容器启动后到就绪 | 约 1 ms | 215 ms | 162 ms | 885 ms | 3.4 s | 11.0 s |
| 空闲内存 | 9.2 MiB | 50 MiB | 68 MiB | 68 MiB | 271 MiB | 156 MiB |

不经过 Docker 直接启动进程，fakes3 从启动到第一个 ListBuckets 返回只要 2 ms，MinIO 是 201 ms。

**模拟测试套件**：100 个测试，共 6,500 个串行请求（建 bucket、读写小对象、List、Copy、删除）

| | 总耗时 | 每请求 |
|---|---:|---:|
| **fakes3** | **0.88 s** | **135 µs** |
| MinIO | 3.0 s | 459 µs |
| LocalStack | 5.7 s | 871 µs |
| S3Mock | 6.6 s | 1,015 µs |
| moto | 13.8 s | 2,120 µs |

**吞吐**（用 MinIO 的 [warp](https://github.com/minio/warp) 压测）：4 KiB 的 GET、PUT、HEAD、List 都是 MinIO 的
3.6～3.9 倍，是 LocalStack 和 moto 的 18～97 倍；64 MiB GET 约 9.3 GB/s，是 MinIO 的 1.8 倍。
64 MiB 大对象 PUT 目前和 MinIO 基本持平，瓶颈在计算 ETag 用的 MD5。

**内存**：写入同样 551 MiB 的数据后，fakes3 的额外开销是 50 MiB，MinIO 是 329 MiB。

## 设计

- **元数据**全部在内存里：每个 bucket 一个有序 map（key → 版本链），列举就是有序遍历。
- **对象数据**是一组不可变、带引用计数的 `Bytes` 分段。小对象放在堆上；大对象写进 `memfd`
  （tmpfs 承载、不占用文件名、进程退出时自动释放），再通过只读 mmap 提供读取。
- 因为数据不可变，GET、CopyObject、UploadPartCopy 和 CompleteMultipartUpload 都**不拷贝对象数据**，
  只是拼接或切片分段、增加引用计数。
- 协议层（路由、XML、SigV4、aws-chunked）由 s3s 负责；这个仓库实现的是存储引擎和 S3 语义。

## 开发

```bash
cargo test                 # 单元测试
cargo clippy --all-targets
```

两个用 `workflow_dispatch` 手动触发的 GitHub Actions workflow：

- **matrix**：先跑单元测试，再对选定的服务跑 SDK 兼容矩阵，报告显示在 job summary 里。
- **bench**：对选定的服务跑启动、模拟测试套件、内存占用和 warp 压测。

本地也可以跑，前提是装好 Docker 和各语言的工具链：

```bash
bench/setup.sh && uv run bench/run.py      # 性能测试，结果写到 bench/RESULTS.md
SERVERS="fakes3 minio" matrix/run.sh && python3 matrix/report.py   # 兼容矩阵
```

`bench/servers.sh` 负责以相同条件启动所有被测实现，也可以单独用来起一个竞品做对比。
