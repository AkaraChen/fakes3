# 兼容性与性能

## 兼容性

[matrix/](../matrix/) 里有一个 SDK 兼容矩阵：**8 种语言、24 个 SDK，每个跑 43 个场景**，同时对 fakes3 和 5 个常见实现跑一遍。
场景定义见 [matrix/SPEC.md](../matrix/SPEC.md)，完整结果见 [matrix/MATRIX.md](../matrix/MATRIX.md)。

| 实现 | 通过率 |
|---|---:|
| **fakes3** | **99.1%** |
| MinIO | 96.4% |
| LocalStack | 91.4% |
| moto | 87.9% |
| S3Mock | 85.4% |
| JuiceFS gateway | 69.3% |

fakes3 剩下的 8 个失败全部是 SDK 自身的 bug（minio-dotnet 有 6 个，minio-java 和 obstore 处理不了 `a//b` 这种 key），换哪个服务都会失败。

另外用独立的一致性测试集 [ceph/s3-tests](https://github.com/ceph/s3-tests)（约 1000 个用例）衡量。下面只统计 AWS 本身能通过、且属于 S3 范围的 684 个用例，
IAM、STS、SNS、S3 Control 属于其他 AWS 服务，不计入：

| 实现 | 通过 | 通过率 |
|---|---:|---:|
| **fakes3** | **587** | **85.8%** |
| LocalStack 4.14 | 404 | 59.1% |
| MinIO | 356 | 52.0% |

fakes3 剩下的 94 个失败里，82 个是测试本身不兼容 AWS（只符合 Ceph RGW 的行为，或者 boto3 测不出来），
5 个是没有实现的 bucket 访问日志，3 个在 s3s 内部，4 个的 AWS 行为没有文档可查。
逐条分类见 [conformance/S3-TESTS.md](../conformance/S3-TESTS.md)，可以用 `s3-tests` workflow 重新跑。

fakes3 在 s3s 之上绕开了几处和 AWS 不一致的行为（SigV2 的日期和 bucket 级签名、POST policy 的错误码等），见 [docs/s3s-workarounds.md](s3s-workarounds.md)。

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
[这次运行的 job summary](https://github.com/AkaraChen/fakes3/actions/runs/41259060444) 里，可以用 `bench` workflow 重新跑。

**启动和空闲内存**

| | fakes3 | MinIO | JuiceFS | moto | S3Mock | LocalStack |
|---|---:|---:|---:|---:|---:|---:|
| 容器启动后到就绪 | 约 1 ms | 243 ms | 169 ms | 880 ms | 3.8 s | 10.9 s |
| 空闲内存 | 9.2 MiB | 51 MiB | 68 MiB | 69 MiB | 233 MiB | 156 MiB |

不经过 Docker 直接启动进程，fakes3 从启动到第一个 ListBuckets 返回约 4 ms，MinIO 是 234 ms。

**模拟测试套件**：100 个测试，共 6,500 个串行请求（建 bucket、读写小对象、List、Copy、删除）

| | 总耗时 | 每请求 |
|---|---:|---:|
| **fakes3** | **0.93 s** | **143 µs** |
| MinIO | 3.4 s | 526 µs |
| S3Mock | 7.1 s | 1,092 µs |
| LocalStack | 7.7 s | 1,180 µs |
| moto | 12.9 s | 1,983 µs |

**吞吐**（用 MinIO 的 [warp](https://github.com/minio/warp) 压测，数字是 fakes3 相对对方的倍数）

| 负载 | fakes3 | MinIO | LocalStack | moto | S3Mock | JuiceFS |
|---|---:|---:|---:|---:|---:|---:|
| 4 KiB GET | 30,045 ops/s | 3.6× | 32× | 56× | 15× | 12× |
| 4 KiB PUT | 18,068 ops/s | 3.7× | 19× | 37× | 55× | 24× |
| HEAD | 40,333 ops/s | 3.6× | 39× | 73× | 18× | 13× |
| ListObjects | 1,380 ops/s | 2.7× | 10× | 32× | 4.3× | 9.6× |
| 64 MiB GET | 6.1 GB/s | 1.3× | 9.0× | 2.0× | 2.1× | 2.1× |
| 64 MiB PUT | 1.0 GB/s | 1.1× | 1.6× | 3.6× | 1.0× | 1.1× |

- 大对象 PUT 的瓶颈是计算 ETag 用的 MD5（已经用了 OpenSSL 的汇编实现，并和接收数据流水线并行）。
- ListObjects 每个 key 都返回 `Owner`，响应比不返回时大约 30%，这是按 AWS 的规定保留的字段。

**内存**：写入同样 551 MiB 的数据后，fakes3 的额外开销约 100 MiB，MinIO 是 344 MiB，S3Mock 是 3.5 GiB。

每个请求的 CPU 开销从哪里来、哪些优化试过但无效，见 [docs/perf-attribution.md](perf-attribution.md)。
