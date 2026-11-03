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

## macOS 和 Windows

fakes3 在 macOS 和 Windows 上原生运行，CI 在三个系统上都跑全部测试。和 Linux 的区别只有两处，都用 `cfg` 隔开：
大对象在 Linux 上写进 memfd，在其他系统上留在堆上（[src/spill.rs](../src/spill.rs)）；`--memory-limit auto` 在 macOS 上读
`hw.memsize`，在 Windows 上读 `GlobalMemoryStatusEx`（[src/memory.rs](../src/memory.rs)）。

macOS 没有 Docker，Windows 跑不了 Linux 容器，所以这两个系统上的性能用 `bench-native` workflow 测（[bench/native.py](../bench/native.py)）：
fakes3、MinIO、moto 都作为本机进程运行，负载和上面的 Docker 测试相同，CPU 和内存取自服务端的进程树。MinIO 和 warp 已经不发布二进制，
都用 `go install` 从源码构建。下面是[一次运行](https://github.com/AkaraChen/fakes3/actions/runs/41601919891)在三个托管 runner 上的结果。
runner 每次分到的 CPU 不一样，不同系统之间的绝对值不能直接比，要看同一台机器上的倍数（括号里是 fakes3 相对 MinIO、moto 的倍数）。

| | Linux | macOS | Windows |
|---|---:|---:|---:|
| runner | EPYC 7763，4 核 | Apple M1（虚拟机），3 核 | EPYC 9V74，4 核 |
| 启动到就绪：fakes3 / MinIO / moto | 3.7 ms / 357 ms / 407 ms | 16 ms / 455 ms / 36 s¹ | 32 ms / 456 ms / 542 ms |
| 空闲内存 | 15.9 MiB | 4.9 MiB | 7.3 MiB |
| 模拟测试套件，每请求 | 245 µs（3.2×，9.8×） | 121 µs（5.9×，20×） | 247 µs（31×，14×） |
| 4 KiB GET，16 并发 | 16,291 ops/s（3.3×，37×） | 27,502 ops/s（3.3×，43×） | 12,574 ops/s（2.0×，42×） |
| 4 KiB PUT，16 并发 | 9,412 ops/s（2.8×，24×） | 14,610 ops/s（6.8×，37×） | 6,040 ops/s（9.9×，21×） |
| HEAD，16 并发 | 20,103 ops/s（3.1×，44×） | 35,299 ops/s（3.6×，68×） | 22,506 ops/s（2.8×，76×） |
| ListObjects，8 并发 | 1,052 ops/s（3.3×，34×） | 1,966 ops/s（6.5×，36×） | 1,343 ops/s（12×，35×） |
| 64 MiB GET，4 并发 | 5.3 GiB/s（1.5×，2.5×） | 6.6 GiB/s（2.0×，2.8×） | 7.1 GiB/s（1.7×，2.5×） |
| 64 MiB GET，单流 | 2.7 GiB/s（1.8×，3.5×） | 5.1 GiB/s（2.9×，2.5×） | 1.7 GiB/s（1.6×，1.6×） |
| 64 MiB PUT，4 并发 | 856 MiB/s（1.2×，4.5×） | 454 MiB/s（1.3×，3.1×） | 797 MiB/s（2.8×，22×） |
| 写入 551 MiB 数据集后的额外内存 | –² | 63 MiB | 42 MiB |

¹ 这次运行里 moto 在 macOS 上每次启动都要 36 秒，前一次运行是 0.6 秒，原因没查。
² Linux 上大对象在 memfd 里，不计入进程的 RSS，这一项看上面的 Docker 测试。MinIO 的数据在磁盘上（Linux 上在 /dev/shm），也不计入。

### 每个请求的 CPU 花在哪

`bench/ab.py` 报告服务端 CPU 里内核态所占的比例。同一个版本自己对比两轮（[Linux](https://github.com/AkaraChen/fakes3/actions/runs/41597775161)、
[macOS](https://github.com/AkaraChen/fakes3/actions/runs/41597770708)、[Windows](https://github.com/AkaraChen/fakes3/actions/runs/41597767517)）：

| | Linux（Xeon 8370C） | macOS（M1 虚拟机） | Windows（EPYC 9V45） |
|---|---:|---:|---:|
| 4 KiB GET，16 并发：服务端 CPU / 请求 | 38 µs | 23–43 µs | 33–35 µs |
| 其中内核态 | 33% | 53% | 47% |
| 4 KiB PUT，16 并发：服务端 CPU / 请求 | 79 µs（内核态 25%） | 52–94 µs（40%） | 82 µs（47%） |

用 [samply](https://github.com/mstange/samply) 对 4 KiB GET 采样（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41601860321)），Windows 上的 on-CPU 时间：
内核约 56%（ntoskrnl 43%、tcpip 6%、afd 3%），fakes3 可执行文件本身（连同静态链接的 tokio、mio、hyper、s3s）约 35%，
ntdll、vcruntime、ws2_32 这些系统库约 8%。折算到每个请求，fakes3 自己的用户态代码在 Windows 上约 12 µs，和 Linux 同档 CPU 上
（约 20 µs 一个请求，60% 在 fakes3 里）相同；Windows 多出来的开销几乎全在内核：网络栈，以及 ntoskrnl 里独占 12% 的一个函数，
对应 tokio worker 线程在虚拟机里的休眠和唤醒（macOS 上相应的是 `__psynch_cvwait` 和 `kevent`）。所以 Windows 上没有能靠条件编译
去掉的用户态热点。

### 在 macOS 和 Windows 上试过的优化

| 尝试 | 结果 | 结论 |
|---|---|---|
| socket 发送、接收缓冲区设成 4 MiB | macOS：吞吐 B/A 0.84–1.14，服务端 CPU 0.96–0.99（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41593902085)）；Windows：单流 64 MiB GET 1.000，其余都在噪声内（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41593898996)） | 不采用，两个系统默认就会自动调整缓冲区 |
| 大对象放进匿名 mmap（像 memfd 一样不占堆） | 写满 64 MiB 新内存：堆 32–38 ms（macOS）/ 11–13 ms（Windows），匿名映射 56–90 ms / 32 ms（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41596450930)） | 不采用，堆更快 |
| 写入前用 `mlock` 一次性预缺页（macOS） | 50–80 ms / 64 MiB，比直接写还慢 | 不采用 |
| Windows 上用 NASM 构建 OpenSSL 的汇编 | MD5 0.71 GB/s，和 Linux 相同（macOS M1 虚拟机上是 0.49–0.56 GB/s） | release 和 bench 都这样构建；没装 NASM 时 OpenSSL 退回 C 实现 |
| 关连接时 lingering close | 修正：提前返回的错误（比如 NoSuchBucket）在 macOS、Windows 上不再变成 connection reset | 所有系统都启用（[src/linger.rs](../src/linger.rs)） |

### 已知瓶颈

- Windows 上单流读大对象约 1.5–1.7 GiB/s（4 路并发能到 7 GiB/s）。这时 fakes3 只用 0.2 核，MinIO、moto 单流也是这个速度，
  限制在客户端和 Windows 回环的单连接吞吐上；调大服务端的 socket 缓冲区没有变化。
- macOS runner 上 64 MiB PUT 约 450 MiB/s，fakes3 用不到 1 核，MinIO 也是这个速度：3 核的机器上 warp 客户端先到了上限。
  服务端每 64 MiB 的主要开销是 MD5（M1 虚拟机上约 130 ms，在单独的线程上流水线计算），其次是首次写入新内存的缺页（约 30 ms）。
