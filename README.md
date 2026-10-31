# fakes3

一个专门给测试用的 S3 服务：全内存、无持久化，启动只要几毫秒，请求延迟在百微秒级。
协议层基于 [s3s](https://github.com/Nugine/s3s)，用 Rust 编写。

它放弃了测试用不到的东西（持久化、fsync、多节点、静态加密），换来启动速度和请求延迟；
但在客户端看得到的地方，尽量和 AWS S3 保持一致：签名、错误码、ETag 规则、版本控制、条件请求、校验和等等。

## 快速开始

用 Docker：

```bash
docker build -t fakes3 .
docker run --rm -p 9000:9000 fakes3
```

镜像基于 Alpine，里面只有一个静态链接的 musl 二进制，压缩后约 7 MB。

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

## 配置

| 参数 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| `--host` | `FAKES3_HOST` | `127.0.0.1`（镜像里是 `0.0.0.0`） | 监听地址 |
| `--port` | `FAKES3_PORT` | `9000` | 监听端口，`0` 表示随机端口 |
| `--access-key` | `FAKES3_ACCESS_KEY` | `fakes3` | 访问凭证 |
| `--secret-key` | `FAKES3_SECRET_KEY` | `fakes3secret` | 访问凭证 |
| `--no-auth` | | 关 | 不校验签名 |
| `--inline-threshold` | | `1048576` | 达到这个大小的对象或 part 存进 memfd，小于它的放在堆上 |

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

还没做的功能见 [TODO.md](TODO.md)，主要是 ACL/Policy、CORS、Lifecycle，以及测试控制面
（reset、快照、虚拟时钟、故障注入）。

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
