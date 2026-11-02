# fakes3

[![ci](https://github.com/AkaraChen/fakes3/actions/workflows/ci.yml/badge.svg)](https://github.com/AkaraChen/fakes3/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/fakes3-sdk)](https://www.npmjs.com/package/fakes3-sdk)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[English](README.md) | 简体中文

**一个专门给测试用的 S3 服务。** 几毫秒就绪，一个请求约 150 µs，在客户端看得到的地方尽量和 AWS S3 保持一致。

它放弃了测试用不到的东西（持久化、fsync、多节点、静态加密），换来启动速度和请求延迟；
客户端能观察到的行为则紧跟 AWS：签名、错误码、ETag 规则、版本控制、条件请求、校验和、ACL 和 Bucket Policy。
用 Rust 编写，协议层基于 [s3s](https://github.com/Nugine/s3s)。

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3
```

## 为什么用 fakes3

- **快到可以每个测试起一个。** 容器启动后约 1 ms 就绪，空闲内存 9 MiB。一个 6,500 个请求的测试套件跑完只要 0.93 s，
  MinIO 要 3.4 s，LocalStack 要 7.7 s。
- **行为和 AWS 一致。** 24 个 SDK 的兼容矩阵通过 99.1%；[ceph/s3-tests](https://github.com/ceph/s3-tests) 里 AWS 自己能通过的用例，
  fakes3 通过 85.8%，LocalStack 59.1%，MinIO 52.0%。
- **为测试设计。** 测试之间重置状态、保存和回滚测试数据、拨动虚拟时钟让预签名 URL 过期或触发 Lifecycle 规则、
  用请求日志断言代码发了哪些请求。
- **跑在你的测试所在的地方。** Docker 镜像（amd64 和 arm64）、在 Node.js 进程内运行、在 Rust 进程内运行、静态链接的 Linux 二进制，或者 GitHub Action。

| | fakes3 | MinIO | LocalStack | moto | S3Mock |
| --- | ---: | ---: | ---: | ---: | ---: |
| 容器启动后到就绪 | **约 1 ms** | 243 ms | 10.9 s | 880 ms | 3.8 s |
| 空闲内存 | **9.2 MiB** | 51 MiB | 156 MiB | 69 MiB | 233 MiB |
| 6,500 个请求的测试套件 | **0.93 s** | 3.4 s | 7.7 s | 12.9 s | 7.1 s |
| SDK 兼容矩阵（24 个 SDK × 43 个场景） | **99.1%** | 96.4% | 91.4% | 87.9% | 85.4% |
| ceph/s3-tests（AWS 能通过的 684 个用例） | **85.8%** | 52.0% | 59.1% | | |

在 4 核的 GitHub 托管 runner 上测量。测量方法、吞吐数据和完整结果见 [docs/benchmarks.md](docs/benchmarks.md)。

## 快速开始

### Docker

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3
```

镜像是 `ghcr.io/akarachen/fakes3`，支持 `linux/amd64` 和 `linux/arm64`，tag 有 `latest`、`0.1.0`、`0.1` 和 `sha-<提交>`。
它基于 Alpine，里面只有一个静态链接的二进制（压缩后约 7 MB），监听 `0.0.0.0:9000`，就绪后在 stdout 打印 `http://0.0.0.0:9000`。

任何 S3 客户端用 path-style 寻址连上即可，凭证随意：

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

### Node.js（进程内）

[`fakes3-sdk`](https://www.npmjs.com/package/fakes3-sdk) 直接在 Node 进程里运行服务，不起子进程，也不需要 Docker：

```bash
npm install --save-dev fakes3-sdk
```

```ts
import { start } from "fakes3-sdk";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 127.0.0.1 的随机空闲端口
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

预编译产物覆盖 Linux 的 x64 和 arm64，glibc 和 musl（Alpine）都有，需要 Node.js 18 或更新版本。
全部选项和方法见[包的 README](bindings/node/README.zh-CN.md)。

### Rust（进程内）

还没有发布到 crates.io，用 git 依赖：

```toml
[dev-dependencies]
fakes3 = { git = "https://github.com/AkaraChen/fakes3", tag = "v0.1.0" }
```

```rust
let server = fakes3::Server::start(fakes3::Config::default()).await?;
let endpoint = server.endpoint(); // "http://127.0.0.1:<随机端口>"
// 用 endpoint 创建 S3 客户端，开启 path-style
server.shutdown().await;
```

同步测试用 `Server::start_blocking`：服务跑在自己的后台 runtime 上，句柄 drop 时自动停止。`Config::default()` 绑定随机端口，
所以并行的测试不会冲突；每个 `Server` 有独立的数据和时钟。`shutdown()` 返回时端口已经可以复用，已建立的连接会继续服务到客户端断开。例子见 [examples/aws_sdk.rs](examples/aws_sdk.rs) 和
[examples/blocking.rs](examples/blocking.rs)。

### 独立二进制

每个 [release](https://github.com/AkaraChen/fakes3/releases) 都附带 x86_64 和 aarch64 的静态 Linux 二进制，各带 SHA-256 校验文件：

```bash
curl -L https://github.com/AkaraChen/fakes3/releases/latest/download/fakes3-x86_64-unknown-linux-musl.tar.gz | tar xz
./fakes3-x86_64-unknown-linux-musl/fakes3 --port 0   # 0 表示随机选一个空闲端口，实际地址打印到 stdout
```

从源码构建：`cargo build --release`。构建时会编译 OpenSSL，需要 `perl` 和 `make`。

## Testcontainers

不需要专门的模块：用通用容器，暴露 9000 端口，等日志里出现 `http://`。容器几毫秒就能启动，所以可以每个测试类、甚至每个测试起一个。

```python
# Python：testcontainers-python
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs

with DockerContainer("ghcr.io/akarachen/fakes3:latest").with_exposed_ports(9000) as c:
    wait_for_logs(c, "http://")
    endpoint = f"http://{c.get_container_host_ip()}:{c.get_exposed_port(9000)}"
```

```ts
// Node.js：testcontainers
import { GenericContainer, Wait } from "testcontainers";

const container = await new GenericContainer("ghcr.io/akarachen/fakes3:latest")
  .withExposedPorts(9000)
  .withWaitStrategy(Wait.forLogMessage("http://"))
  .start();
const endpoint = `http://${container.getHost()}:${container.getMappedPort(9000)}`;
```

```java
// Java：Testcontainers for Java
GenericContainer<?> s3 = new GenericContainer<>("ghcr.io/akarachen/fakes3:latest")
    .withExposedPorts(9000)
    .waitingFor(Wait.forLogMessage(".*http://.*\\n", 1));
s3.start();
String endpoint = "http://" + s3.getHost() + ":" + s3.getMappedPort(9000);
```

```go
// Go：testcontainers-go
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

## GitHub Actions

[`setup-fakes3`](https://github.com/AkaraChen/setup-fakes3) 在 Linux runner 上安装 fakes3、在后台启动，并把 `fakes3 ctl` 放进 `PATH` 供后续步骤使用。
它不需要 Docker，所以在 container job 里也能用：

```yaml
- uses: AkaraChen/setup-fakes3@v1
  id: s3
  with:
    version: 0.1.0      # 默认 latest
- run: pytest
  env:
    S3_ENDPOINT: ${{ steps.s3.outputs.endpoint }}   # http://127.0.0.1:9000
```

输入项和[配置](#配置)里的选项一一对应；`port: 0` 随机选一个空闲端口，`aws-env: true` 导出 `AWS_ENDPOINT_URL_S3` 和凭证，AWS CLI 和各语言 SDK 直接就能用。
全部输入和输出见 [action 的 README](https://github.com/AkaraChen/setup-fakes3#readme)。

也可以把 Docker 镜像作为 service container：

```yaml
services:
  s3:
    image: ghcr.io/akarachen/fakes3:0.1.0
    ports: ["9000:9000"]
```

## 测试控制面

同一个端口上的 `/_fakes3/` 前缀是一组 JSON API（bucket 名不能以 `_` 开头，所以不会和 S3 冲突），用来把服务放到已知状态、检查代码做了什么。
`fakes3 ctl` 是它的命令行客户端：

```bash
fakes3 ctl reset                              # 清空所有 bucket、对象和上传，同时清空请求日志和时钟
fakes3 ctl snapshot create seeded             # 保存当前状态（共享对象数据，不复制）
fakes3 ctl snapshot restore seeded            # 回滚，可以反复回滚
fakes3 ctl clock advance 31d                  # 推进服务器时间，预签名 URL、Lifecycle、Object Lock 都跟着走
fakes3 ctl requests list --op PutObject       # 请求日志：按 bucket、操作、状态码等过滤
```

进程内使用时是服务对象上的方法：Node.js 里是 `server.reset()`、`server.snapshot("seeded")`、`server.clock.advance("31d")`、
`server.requests.list({ operation: "PutObject" })`，Rust 里是 `fakes3::Server` 上对应的方法。端点和语义见 [docs/control-plane.md](docs/control-plane.md)。

## 配置

每个选项都可以用命令行参数或环境变量设置：

| 参数 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--host` | `FAKES3_HOST` | `127.0.0.1`（镜像里是 `0.0.0.0`） | 监听地址 |
| `--port` | `FAKES3_PORT` | `9000` | 监听端口，`0` 表示随机空闲端口 |
| `--access-key` / `--secret-key` | `FAKES3_ACCESS_KEY` / `FAKES3_SECRET_KEY` | `fakes3` / `fakes3secret` | 默认账户的凭证，只在 `--auth` 时校验 |
| `--auth` | `FAKES3_AUTH` | 关 | 校验 SigV4 / SigV2 签名（见[认证](#认证)） |
| `--user` | `FAKES3_USERS` | | 再加一个账户：`name:access-key:secret[:account-id[:canonical-id]]`，可重复（环境变量里用逗号分隔） |
| `--domain` | `FAKES3_DOMAIN` | `localhost` | virtual-host 寻址的基础域名（`<bucket>.<domain>`），可重复，传 `""` 关闭 |
| `--bucket-defaults` | `FAKES3_BUCKET_DEFAULTS` | `legacy` | 新建 bucket 的初始设置：`legacy` 或 `aws`（见下文） |
| `--memory-limit` | `FAKES3_MEMORY_LIMIT` | `auto` | 对象数据的内存上限：`512MiB`、`2GiB`、`unlimited`，或 `auto`（容器或机器内存的 75%） |
| `--inline-threshold` | | `1048576` | 达到这个大小的对象存进 `memfd`，小于它的放在堆上 |
| `--no-request-log` | | | 不记录请求日志 |
| `--request-log-capacity` | | `10000` | 请求日志最多保留的条数，满了丢弃最早的 |

### 认证

默认**接受任意凭证**（和 LocalStack、moto 一样），但授权照常生效：

- **带签名**的请求不验签，身份由 access key 决定；没配置过的 key 是默认账户（拥有一切的 owner）。
- **不带签名**的请求是匿名请求，由 ACL、Bucket Policy 和 Block Public Access 决定能做什么，和 AWS 一样，
  所以用 `curl` 访问私有 bucket 会得到 `403 AccessDenied`。
- 加 **`--auth`** 后按配置的凭证校验签名，签名不对返回 403。

两种模式下负载完整性校验都生效（`Content-MD5`、`x-amz-checksum-*`、aws-chunked trailer）。详见 [docs/auth.md](docs/auth.md)。

### 多账户

默认只有一个账户。要测试跨账户的行为（ACL 授权、policy 的 Principal、对象归属、`x-amz-expected-bucket-owner`），用 `--user` 再加几个：

```bash
fakes3 --user alt:ALTKEY:ALTSECRET --user carol:CAROLKEY:CAROLSECRET:111122223333
```

账户 id 和 canonical id 是确定性生成的，测试里可以硬编码。见 [docs/multi-user.md](docs/multi-user.md)。

### 新建 bucket 的默认值

AWS 在 2023 年改了新 bucket 的默认设置。fakes3 默认保留旧行为（`legacy`），这样“先挂一个公开 policy，再匿名读取”之类的准备步骤开箱即用；
`--bucket-defaults aws` 和今天的 AWS 一致：禁用 ACL（`BucketOwnerEnforced`）、开启 Block Public Access、默认 SSE-S3 加密、禁用 SSE-C。
见 [docs/bucket-defaults.md](docs/bucket-defaults.md)。

### 内存上限

数据全在内存里，所以 fakes3 统计每一个字节的对象数据。超出上限的写入返回 `507 Insufficient Storage`，而不是让容器在测试中途被 OOM 杀掉；
请求体大小事先知道时，在读取请求体之前就会拒绝。主流 SDK 都不会重试 507。见 [docs/memory-limit.md](docs/memory-limit.md)。

## 支持的功能

- **Bucket**：创建、删除、HeadBucket、ListBuckets（分页）、GetBucketLocation、标签
- **对象**：PUT、GET、HEAD、DELETE、DeleteObjects、CopyObject（支持替换元数据和标签）、Range 读取、标签、GetObjectAttributes
- **列举**：ListObjectsV2、ListObjects（v1）、ListObjectVersions、ListMultipartUploads，支持 prefix、delimiter、分页、`encoding-type=url`
- **分段上传**：Create、UploadPart、UploadPartCopy、ListParts、Complete、Abort，GET / HEAD 支持 `?partNumber=`
- **版本控制**：Enabled / Suspended、删除标记、null version、按版本读取和删除
- **Object Lock**：GOVERNANCE / COMPLIANCE 保留期、Legal Hold、bucket 默认保留期、绕过 governance
- **条件请求**：`If-Match`、`If-None-Match`（包括条件写入）、`If-Modified-Since`、`If-Unmodified-Since`，以及 CopySource 的条件头
- **校验和**：CRC32、CRC32C、CRC64NVME、SHA-1、SHA-256，header 和 aws-chunked trailer 两种方式，分段上传支持 COMPOSITE 和 FULL_OBJECT；
  默认 CRC64NVME，和现在的 AWS 一致
- **签名**：SigV4（header、预签名 URL、带 trailer 的流式上传）、SigV2、POST policy 表单上传
- **访问控制**：Bucket / Object ACL、Object Ownership、Block Public Access、带 Condition 的 Bucket Policy、匿名访问、多账户
- **加密**：SSE-C 的 key 校验、SSE-S3 / SSE-KMS 头、bucket 默认加密；只模拟行为，数据**不会真的加密**
- **CORS**：预检请求和响应上的 CORS 头
- **Lifecycle**：过期（包括非当前版本和删除标记）、清理未完成的分段上传、存储类转换，全部由虚拟时钟驱动
- **归档存储类**：`GLACIER`、`DEEP_ARCHIVE` 和 Intelligent-Tiering 的归档层，读取返回 `InvalidObjectState`，`RestoreObject` 和
  `x-amz-restore` 按虚拟时钟推进（见 [docs/archive-storage.md](docs/archive-storage.md)）
- **S3 Select**：对 CSV 和 JSON 输入执行 SQL（见 [docs/select.md](docs/select.md)）
- **请求 id**：每个响应都带 `x-amz-request-id` 和 `x-amz-id-2`，和请求日志里的一致

不支持：网站端点、事件通知投递、服务器访问日志、IAM、STS 和 S3 Control。Website 和 Notification 的**配置**可以存取。
后续计划见 [TODO.md](TODO.md)。

## 平台

| | 平台 |
| --- | --- |
| Docker 镜像 | `linux/amd64`、`linux/arm64`（在 macOS 和 Windows 上可以用 Docker Desktop 运行） |
| 静态二进制 | x86_64 和 aarch64 Linux |
| Node.js 包 | Linux x64 和 arm64，glibc 和 musl，Node.js ≥ 18 |
| Rust 库 | Linux |

不支持在 macOS 或 Windows 上原生运行（大对象存放在 Linux 的 `memfd` 里），在这些系统上请用 Docker 镜像。

## 安全提示

fakes3 是给测试用的，**不要把它暴露在不可信的网络上，也不要拿它存真实数据**：

- 默认不校验签名，能连上端口的人都能读写。即使加了 `--auth`，凭证也是明文配置的测试凭证。
- 控制面（`/_fakes3/*`）没有认证，能连上的人可以清空全部数据或读到请求日志；它只拒绝浏览器发起的跨域写请求。
- 二进制默认只监听 `127.0.0.1`，**Docker 镜像默认监听 `0.0.0.0`**。在共享机器上只把端口映射到本机（`-p 127.0.0.1:9000:9000`），或者放在内部网络里。
- 没有持久化，进程退出数据就没了。

## 实现方式

- **元数据**全部在内存里：每个 bucket 一个有序 map（key → 版本链），列举就是有序遍历。
- **对象数据**是一组不可变、带引用计数的 `Bytes` 分段。小对象放在堆上；大对象写进 `memfd`（tmpfs 承载、不占用文件名、进程退出时自动释放），
  再通过只读 mmap 提供读取。
- 因为数据不可变，GET、CopyObject、UploadPartCopy 和 CompleteMultipartUpload 都**不拷贝对象数据**，只是拼接或切片分段、增加引用计数。
- 协议层（路由、XML、SigV4、aws-chunked）由 [s3s](https://github.com/Nugine/s3s) 负责；这个仓库实现的是存储引擎和 S3 语义。

## 文档

| | |
| --- | --- |
| [docs/control-plane.md](docs/control-plane.md) | 重置、快照、虚拟时钟、请求日志：HTTP API、CLI、Rust API |
| [docs/auth.md](docs/auth.md) | 认证模式的细节 |
| [docs/multi-user.md](docs/multi-user.md) | 账户、归属和跨账户授权 |
| [docs/bucket-defaults.md](docs/bucket-defaults.md) | `legacy` 和 `aws` 两种新 bucket 设置，附 AWS 文档出处 |
| [docs/memory-limit.md](docs/memory-limit.md) | 内存怎么统计、怎么限制 |
| [docs/archive-storage.md](docs/archive-storage.md) | Glacier、Deep Archive、Intelligent-Tiering、恢复、Lifecycle 转换 |
| [docs/select.md](docs/select.md) | S3 Select 的覆盖范围 |
| [docs/benchmarks.md](docs/benchmarks.md) | SDK 兼容矩阵、ceph/s3-tests 和性能测试结果 |
| [docs/s3s-workarounds.md](docs/s3s-workarounds.md) | fakes3 在哪些地方修正了 s3s 以对齐 AWS |
| [docs/development.md](docs/development.md) | 构建、测试、CI 和发布 |

## 参与开发

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

每次 push 和 PR 都会在 CI 里跑这些检查。SDK 兼容矩阵、ceph/s3-tests 和性能测试是手动触发的 GitHub Actions workflow，见 [docs/development.md](docs/development.md)。

## 许可证

[MIT](LICENSE)
