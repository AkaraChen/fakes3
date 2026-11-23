# 兼容性与性能

## 兼容性

[matrix/](../matrix/) 里有一个 SDK 兼容矩阵：**8 种语言、24 个 SDK，每个跑 43 个场景**，同时对 fakes3 和 5 个常见实现跑一遍。
场景定义见 [matrix/SPEC.md](../matrix/SPEC.md)，完整结果见 [matrix/MATRIX.md](../matrix/MATRIX.md)。
fakes3 0.1.4 的结果来自[这次运行](https://github.com/AkaraChen/fakes3/actions/runs/45109530162)；其他实现的结果来自同一套场景之前的运行。

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
逐条分类见 [conformance/S3-TESTS.md](../conformance/S3-TESTS.md)，可以用 `s3-tests` workflow 重新跑；fakes3 0.1.4 的结果来自[这次运行](https://github.com/AkaraChen/fakes3/actions/runs/45109533046)。

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

下面的数字都是 fakes3 **0.1.4** 的，每项在 GitHub Actions 的 4 核托管 runner 上跑 3 轮（3 次独立的 workflow 运行，分到的机器不同），
表里写 3 轮的中位数，括号里是 3 轮中的最小值–最大值。托管 runner 之间的速度能差 2 倍多，所以绝对值只能当量级看；
和其他实现比的「倍数」每轮都是在同一台机器上算的，再取 3 轮的中位数。每次运行的完整报告在它的 job summary 和 artifact（`bench/RESULTS.md`）里。

| 测量 | workflow | 3 轮运行 |
|---|---|---|
| Docker 对比（本节前半） | `bench` | [1](https://github.com/AkaraChen/fakes3/actions/runs/45109514746)、[2](https://github.com/AkaraChen/fakes3/actions/runs/45109519965)、[3](https://github.com/AkaraChen/fakes3/actions/runs/45109525006) |
| 原生进程，Linux / macOS / Windows | `bench-native` | [1](https://github.com/AkaraChen/fakes3/actions/runs/45109517429)、[2](https://github.com/AkaraChen/fakes3/actions/runs/45109522377)、[3](https://github.com/AkaraChen/fakes3/actions/runs/45109527334) |
| 进程内启动 | `bench-native`（`only: startup_cycle`） | [1](https://github.com/AkaraChen/fakes3/actions/runs/45109981260)、[2](https://github.com/AkaraChen/fakes3/actions/runs/45109983677)、[3](https://github.com/AkaraChen/fakes3/actions/runs/45109986059) |

### Docker

所有实现都跑在 Docker 里，使用 host 网络，数据目录放在 tmpfs 上，每项测量都用一个新启动的容器。
3 轮里两轮分到 Xeon 6975P-C，一轮分到 EPYC 9V74（慢一些，括号里的下限大多来自这一轮）。

**启动和空闲内存**

| | fakes3 | MinIO | JuiceFS | moto | S3Mock | LocalStack |
|---|---:|---:|---:|---:|---:|---:|
| 容器启动后到就绪 | 0.9 ms（0.8–1.0） | 255 ms | 169 ms | 873 ms | 3.9 s | 11.0 s |
| 空闲内存 | 9.5 MiB（9.4–11.2） | 51 MiB | 67 MiB | 68 MiB | 274 MiB | 156 MiB |

「容器启动后」从 `docker run -d` 返回算起，去掉了每个实现都要付的 Docker 开销。不经过 Docker 直接启动进程，
fakes3 从启动到第一个 ListBuckets 返回是 3.1 ms（3.0–3.2），MinIO 是 229 ms（226–240）。

**模拟测试套件**：100 个测试，共 6,500 个串行请求（建 bucket、读写小对象、List、Copy、删除）

| | 总耗时 | 每请求 | fakes3 快多少 |
|---|---:|---:|---:|
| **fakes3** | **0.90 s**（0.89–1.18） | **138 µs**（138–181） | |
| MinIO | 3.4 s | 528 µs | 3.8×（3.3–3.8） |
| S3Mock | 7.0 s | 1,069 µs | 7.8×（5.7–8.0） |
| LocalStack | 7.9 s | 1,214 µs | 8.8×（6.5–9.0） |
| moto | 13.2 s | 2,024 µs | 15×（13–15） |

**吞吐**（用 MinIO 的 [warp](https://github.com/minio/warp) 压测，数字是 fakes3 相对对方的倍数）

| 负载 | fakes3 | MinIO | LocalStack | moto | S3Mock | JuiceFS |
|---|---:|---:|---:|---:|---:|---:|
| 4 KiB GET，16 并发 | 29,554 ops/s（25,524–29,929） | 3.6× | 32× | 57× | 15× | 12× |
| 4 KiB PUT，16 并发 | 17,992 ops/s（15,754–18,748） | 4.0× | 20× | 39× | 54× | 24× |
| HEAD，16 并发 | 42,332 ops/s（33,402–42,362） | 3.8× | 42× | 77× | 19× | 13× |
| ListObjects，8 并发 | 1,383 ops/s（1,229–1,397） | 2.8× | 11× | 33× | 4.3× | 9.6× |
| 64 MiB GET，4 并发 | 6.2 GiB/s（6.1–6.5） | 1.4× | 9.3× | 2.2× | 2.1× | 2.2× |
| 64 MiB PUT，4 并发 | 1,015 MiB/s（1,006–1,043） | 1.15× | 1.9× | 4.0× | 1.1× | 1.1× |

- 大对象 PUT 的瓶颈是计算 ETag 用的 MD5（已经用了 OpenSSL 的汇编实现，并和接收数据流水线并行）。
- ListObjects 每个 key 都返回 `Owner`，响应比不返回时大约 30%，这是按 AWS 的规定保留的字段。

**内存**：写入同样 551 MiB 的数据后，fakes3 的额外开销约 106 MiB（104–107），MinIO 是 328 MiB，S3Mock 是 3.5 GiB（1.3–3.9 GiB）。

每个请求的 CPU 开销从哪里来、哪些优化试过但无效，见 [docs/perf-attribution.md](perf-attribution.md)。

### 进程内启动：每个测试起一个要多少时间

Node 包和 Rust crate 把服务端跑在测试进程里面，不需要 Docker，也不需要另起进程。[bench/startup](../bench/startup/) 测的是一个测试
为自己的 fakes3 付出的全部时间：启动一个服务、发一个请求（带签名的 ListBuckets，新建连接）、停止它。测的是发布出去的版本：
npm 上的 `fakes3-sdk@0.1.4`、crates.io 上的 `fakes3 = "=0.1.4"`，以及 GitHub Release 里 v0.1.4 的二进制（Linux 上是 musl 静态链接版）。

- **Rust `Server::start`**：在测试已有的 Tokio runtime（多线程）上启动，`shutdown().await` 停止。
- **Rust `Server::start_blocking`**：同步测试用，每个服务新建自己的 runtime，`shutdown()` 停止。
- **Node `start()`**：Node 24，`await server.stop()` 停止；请求用 `node:http`。
- **二进制子进程**：`fakes3 --port 0`，读到它打印的地址就算就绪，结束时直接杀掉进程并等它退出。

每轮运行里先热身 10 次，再连续跑 200 次，取中位数；表里是 3 轮运行的中位数（最小值–最大值）。
runner 和上面 `bench-native` 是同一类（Linux、Windows 4 核 x86，macOS 3 核 M1 虚拟机）。

| 启动 + 一次请求 + 停止 | Linux | macOS | Windows |
|---|---:|---:|---:|
| Rust `Server::start` | **0.15 ms**（0.13–0.24） | **0.26 ms**（0.18–0.33） | **0.53 ms**（0.51–0.68） |
| Rust `Server::start_blocking` | 0.97 ms（0.65–1.03） | 0.46 ms（0.36–0.60） | 0.83 ms（0.83–1.05） |
| Node `start()` | **0.43 ms**（0.34–0.58） | **0.55 ms**（0.30–0.58） | **0.71 ms**（0.67–0.96） |
| 二进制，子进程 | 3.4 ms（2.3–3.5） | 4.7 ms（3.5–4.8） | 10.0 ms（8.2–13.2） |

- 进程内的一轮里（`start_blocking` 除外），启动本身只占 0.02–0.2 ms（绑定端口、建好空的存储），大部分时间是那一个请求：新建 TCP 连接加一次往返。
  子进程的时间主要花在创建进程上：从启动到打印出地址，Linux 2.6 ms（1.7–2.7），macOS 3.8 ms（2.6–3.9），Windows 7.8 ms（6.5–10.1）。
- 一次性的开销不在表里：Node 第一次 `import "fakes3-sdk"` 要 3–6 ms（加载原生扩展），第一轮启动 + 请求 + 停止要 4–10 ms；
  macOS 上第一次执行刚下载的二进制，这一轮要 24–47 ms，之后每次 3.5–5 ms。
- `start_blocking` 每次要新建一个多线程 runtime，在 Linux 上比 `Server::start` 多 0.4–0.7 ms；已经在 async 测试里的话用 `Server::start`。

**同时起几百个**：同一个进程里同时发起 N 轮「启动 + 一次请求 + 停止」，从发起到全部结束的时间（每轮运行 5 次取中位数，再取 3 轮的中位数和范围）：

| | Linux | macOS | Windows |
|---|---:|---:|---:|
| Rust `Server::start`，100 个 | 4.5 ms（3.3–7.8） | 8.1 ms（3.8–10.4） | 22 ms（21–26） |
| Rust `Server::start`，500 个 | 26 ms（16–35） | 59 ms（29–64） | 138 ms（129–165） |
| Node `start()`，100 个 | 21 ms（14–37） | 16 ms（15–44） | 57 ms（47–72） |
| Node `start()`，500 个 | 84 ms（57–140） | 97 ms（68–113） | 254 ms（230–293） |
| 二进制子进程，100 个 | 112 ms（94–122） | 149 ms（115–154） | 403 ms（287–432） |
| 二进制子进程，250 个 | 282 ms（238–300） | 394 ms（269–503） | 1,006 ms（734–1,121） |

一个 Node 进程里同时开着 500 个服务（各处理过一个请求），常驻内存只多 38 MiB（Linux；macOS 41 MiB，Windows 29 MiB），
每个约 80 KiB。并行的测试 worker 各自起服务、端口互不冲突（都是随机空闲端口），不需要在测试之间共享一个服务。

## macOS 和 Windows

fakes3 在 macOS 和 Windows 上原生运行，CI 在三个系统上都跑全部测试。和 Linux 的区别只有两处，都用 `cfg` 隔开：
大对象在 Linux 上写进 memfd，在其他系统上留在堆上（[src/spill.rs](../src/spill.rs)）；`--memory-limit auto` 在 macOS 上读
`hw.memsize`，在 Windows 上读 `GlobalMemoryStatusEx`（[src/memory.rs](../src/memory.rs)）。

macOS 没有 Docker，Windows 跑不了 Linux 容器，所以这两个系统上的性能用 `bench-native` workflow 测（[bench/native.py](../bench/native.py)）：
fakes3、MinIO、moto 都作为本机进程运行，负载和上面的 Docker 测试相同，CPU 和内存取自服务端的进程树。MinIO 和 warp 已经不发布二进制，
都用 `go install` 从源码构建。数字来自[上面列出的](#性能) 3 轮 `bench-native` 运行，写法相同：3 轮的中位数（最小值–最大值）。
runner 每次分到的 CPU 不一样，不同系统之间的绝对值不能直接比，要看同一台机器上的倍数（括号里先是范围，再是 fakes3 相对 MinIO、moto 的倍数，取 3 轮中位数）。

| | Linux | macOS | Windows |
|---|---:|---:|---:|
| runner（3 轮） | Xeon 6975P-C ×2、EPYC 9V74，4 核 | Apple M1（虚拟机），3 核 | Xeon 8573C、EPYC 7763 ×2，4 核 |
| 启动到就绪：fakes3 / MinIO / moto | 3.6 ms / 280 ms / 285 ms | 16 ms / 515 ms / 504 ms | 35 ms / 512 ms / 700 ms |
| 空闲内存 | 15.7 MiB（15.7–18.1） | 4.9 MiB（4.9–5.0） | 7.2 MiB（7.2–7.2） |
| 模拟测试套件，每请求 | 135 µs（132–183；4.0×，12×） | 182 µs（140–199；5.3×，15×） | 311 µs（259–313；19×，15×） |
| 4 KiB GET，16 并发 | 28,415 ops/s（24,699–28,944；3.5×，47×） | 17,541 ops/s（15,850–21,918；3.0×，36×） | 15,216 ops/s（14,861–19,559；3.6×，69×） |
| 4 KiB PUT，16 并发 | 16,376 ops/s（13,969–17,244；3.5×，29×） | 10,217 ops/s（9,239–11,176；7.0×，25×） | 8,911 ops/s（7,877–10,009；15×，40×） |
| HEAD，16 并发 | 39,891 ops/s（32,987–40,428；3.7×，63×） | 23,742 ops/s（23,606–24,545；2.7×，57×） | 18,811 ops/s（18,295–25,425；3.5×，81×） |
| ListObjects，8 并发 | 1,692 ops/s（1,484–1,700；3.1×，33×） | 1,372 ops/s（1,054–1,443；4.8×，25×） | 943 ops/s（923–1,047；13×，36×） |
| 64 MiB GET，4 并发 | 6.4 GiB/s（6.1–6.5；1.4×，2.0×） | 4.7 GiB/s（4.4–6.2；1.6×，1.9×） | 3.7 GiB/s（3.4–4.3；1.2×，1.8×）² |
| 64 MiB GET，单流 | 3.8 GiB/s（3.0–4.0；1.9×，3.2×） | 2.7 GiB/s（2.6–3.7；1.5×，1.7×） | 0.9 GiB/s（0.8–1.1；2.4×，1.5×） |
| 64 MiB PUT，4 并发 | 1,048 MiB/s（1,040–1,052；1.1×，3.1×） | 366 MiB/s（300–385；0.86×，3.2×） | 619 MiB/s（595–641；4.1×，6.3×） |
| 写入 551 MiB 数据集后的额外内存 | –¹ | 60 MiB（51–65） | 44 MiB（41–47） |

启动时间是从启动进程到第一个带签名的 ListBuckets 返回 200，探测本身在 Windows 和 macOS 上有十几毫秒的开销。
不经过探测、只计到进程打印出监听地址，见上面[进程内启动](#进程内启动每个测试起一个要多少时间)一节的子进程数字：Linux 2.6 ms，macOS 3.8 ms，Windows 7.8 ms，
其中大部分是创建进程本身。

¹ Linux 上大对象在 memfd 里，不计入进程的 RSS，这一项看上面的 Docker 测试。MinIO 的数据在磁盘上（Linux 上在 /dev/shm），也不计入。
² EPYC 7763 的 Windows runner 上，64 MiB GET 时有时会进入一种慢的状态：每个对象的服务端 CPU 从约 7.5 ms 变成约 34 ms，九成在内核里，
不论哪个版本都会遇到（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41615044972)），MinIO 和 moto 也一样。EPYC 9V74 的 runner 上
同样的负载是 7 GiB/s 左右。

### 每个请求的 CPU 花在哪

`bench/ab.py` 报告服务端 CPU 里内核态所占的比例。0.1.4 自己和自己交替跑 6 轮（每个负载 12 个数据点：[Linux](https://github.com/AkaraChen/fakes3/actions/runs/45109536106)、
[macOS](https://github.com/AkaraChen/fakes3/actions/runs/45109539373)、[Windows](https://github.com/AkaraChen/fakes3/actions/runs/45109542301)），表里是中位数（最小值–最大值）：

| | Linux（Xeon 8573C） | macOS（M1 虚拟机） | Windows（EPYC 9V74） |
|---|---:|---:|---:|
| 4 KiB GET，16 并发：服务端 CPU / 请求 | 32 µs（31–32） | 36 µs（26–50） | 42 µs（40–45） |
| 其中内核态 | 32%（30–32%） | 52%（51–54%） | 48%（43–53%） |
| 4 KiB PUT，16 并发：服务端 CPU / 请求 | 63 µs（63–63），内核态 24% | 74 µs（63–107），内核态 41% | 84 µs（83–90），内核态 42% |

下面的采样和优化尝试是在 0.1.3 之前的版本上做的。

用 [samply](https://github.com/mstange/samply) 对 4 KiB GET 采样（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41601860321)），Windows 上的 on-CPU 时间：
内核约 56%（ntoskrnl 43%、tcpip 6%、afd 3%），fakes3 可执行文件本身（连同静态链接的 tokio、mio、hyper、s3s）约 35%，
ntdll、vcruntime、ws2_32 这些系统库约 8%。折算到每个请求，fakes3 自己的用户态代码在 Windows 上约 12 µs，和 Linux 同档 CPU 上
（约 20 µs 一个请求，60% 在 fakes3 里）相同；Windows 多出来的开销几乎全在内核：网络栈，以及 ntoskrnl 里独占 12% 的一个函数，
对应 tokio worker 线程在虚拟机里的休眠和唤醒（macOS 上相应的是 `__psynch_cvwait` 和 `kevent`）。所以 Windows 上没有能靠条件编译
去掉的用户态热点；能做的是少唤醒线程：Windows 上 fakes3 只用一半的核做 worker 线程（见下表）。

### 在 macOS 和 Windows 上试过的优化

| 尝试 | 结果 | 结论 |
|---|---|---|
| socket 发送、接收缓冲区设成 4 MiB | macOS：吞吐 B/A 0.84–1.14，服务端 CPU 0.96–0.99（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41593902085)）；Windows：单流 64 MiB GET 1.000，其余都在噪声内（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41593898996)） | 不采用，两个系统默认就会自动调整缓冲区 |
| 大对象放进匿名 mmap（像 memfd 一样不占堆） | 写满 64 MiB 新内存：堆 32–38 ms（macOS）/ 11–13 ms（Windows），匿名映射 56–90 ms / 32 ms（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41596450930)） | 不采用，堆更快 |
| 写入前用 `mlock` 一次性预缺页（macOS） | 50–80 ms / 64 MiB，比直接写还慢 | 不采用 |
| Windows 上用 NASM 构建 OpenSSL 的汇编 | MD5 0.71 GB/s，和 Linux 相同（macOS M1 虚拟机上是 0.49–0.56 GB/s） | release 和 bench 都这样构建；没装 NASM 时 OpenSSL 退回 C 实现 |
| 关连接时 lingering close | 修正：提前返回的错误（比如 NoSuchBucket）在 macOS、Windows 上不再变成 connection reset | 所有系统都启用（[src/linger.rs](../src/linger.rs)） |
| tokio worker 线程数减半（至少 2 个） | Windows（4 核，四次 A/B，两台 EPYC 9V74、两台 EPYC 7763）：并发小请求的吞吐 B/A 1.19–1.34，每请求服务端 CPU 0.64–0.76，每次每轮都一样；单连接延迟、64 MiB GET 和 PUT 持平或略好（[1](https://github.com/AkaraChen/fakes3/actions/runs/41604107640)、[2](https://github.com/AkaraChen/fakes3/actions/runs/41609262547)、[3](https://github.com/AkaraChen/fakes3/actions/runs/41615049618)、[4](https://github.com/AkaraChen/fakes3/actions/runs/41615044972)）。只少一个（核数 − 1）只有 1.05–1.07（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41615256038)）。macOS（3 核 → 2）：0.87–1.35，有好有坏（[运行](https://github.com/AkaraChen/fakes3/actions/runs/41604111663)） | 只在 Windows 上启用（`worker_threads()`，[src/blocking.rs](../src/blocking.rs)），CLI 和 `Server::start_blocking` 都用 |

### 已知瓶颈

- Windows 上单流读大对象约 0.8–1.1 GiB/s（4 路并发 3.4–4.3 GiB/s，EPYC 9V74 的 runner 上能到 7 GiB/s）。这时 fakes3 只用 0.2 核，
  MinIO、moto 单流更慢，限制在客户端和 Windows 回环的单连接吞吐上；调大服务端的 socket 缓冲区没有变化。
- macOS runner 上 64 MiB PUT 约 300–390 MiB/s，fakes3 用不到 1 核，MinIO 也是这个速度（同一次运行里略快一些，fakes3 是它的 0.77–0.95 倍）：
  3 核的机器上 warp 客户端先到了上限。
  服务端每 64 MiB 的主要开销是 MD5（M1 虚拟机上约 130 ms，在单独的线程上流水线计算），其次是首次写入新内存的缺页（约 30 ms）。
