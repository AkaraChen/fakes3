# 大对象 GET 走 sendfile：评估结论

**结论：不做。** 服务端 CPU 确实少了一半，但在 CI 上端到端吞吐没有提升（4 路并发 ±5%，在噪声内），
单流读大对象反而慢了约 37%。实现保留在分支 `exp/sendfile-impl`，没有合入。

## 设计

hyper 不支持 sendfile，s3s 又负责构造响应，所以没有去重写连接处理。分支 `exp/sendfile-impl` 里的做法是在**写 socket 的这一层按地址替换**：

- 大对象写进 memfd 时，把整段 mmap 的 `(起始地址, 长度, memfd)` 登记到一个全局表，owner 销毁时注销
  （owner 放在 `memory::Tracked` 里面，内存记账不变）。
- 包装 hyper 的 IO（`Read + Write`）。`poll_write_vectored` 发现某个 `IoSlice` 落在登记过的映射里且不小于 128 KiB，
  就对这段调用 `sendfile(socket, memfd, offset, len)`；它之前的 slice（响应头）用 `MSG_MORE` 先发出。
  其余写入原样走普通路径。
- 状态行、响应头、`Content-Length`/chunked、keep-alive、pipelining 仍然全由 hyper 处理；Range、`partNumber`、
  多段对象（每段各自是一个映射）由 s3s 决定 body 是什么，所以响应字节和原路径完全一致。
  HTTP/2 的帧只有 16 KiB，不会命中，自然回退。堆上的小对象不受影响。
- 代价：每个大 blob 要一直占一个 fd（用 `RLIMIT_NOFILE` 的一半做预算，超出的 blob 回退到拷贝）；
  对端提前断开时 `sendfile` 会触发 SIGPIPE（Rust 可执行文件默认忽略）。

测试 `tests/sendfile.rs`（在 `exp/sendfile-impl` 上）对比开关两种情况下的整套响应：整体 GET、Range、`partNumber`、
跨 memfd 与堆分段的多段对象、同一连接上 pipeline（中间夹一个 404）、并发读，响应头和 body 逐字节一致。

## 原型：阻塞 socket，回环，16 × 64 MiB memfd 对象

`examples/sendfile_proto.rs`，CI 运行：
[#41180072368](https://github.com/AkaraChen/fakes3/actions/runs/41180072368)（CPU 型号未记录）、
[#41181477918](https://github.com/AkaraChen/fakes3/actions/runs/41181477918)（EPYC 9V45、9V74、Xeon 8370C 各一次）。

- 一次 `write` 整个 64 MiB 的映射比 256 KiB 分块慢得多（7.4 vs 11.5 GB/s）。
- `sendfile` 在这个原型里每个 CPU 上都最快：4 连接约 13～20 GB/s，对比 `write` 的 8～17 GB/s；单连接 5～7 GB/s，对比 2.6～5.7 GB/s；
  服务端每 GB 的 CPU 时间从 0.25～0.46 s 降到 0.07～0.12 s。

## 真实服务：同一台 runner 上对照（warp 作客户端，Docker host 网络）

同一个 job 里先后跑 `--no-sendfile` 和 `sendfile`，MiB/s：

| 运行 | CPU | 4 路 64 MiB：拷贝 → sendfile | 单流 64 MiB：拷贝 → sendfile | 服务端 CPU（4 路） |
|---|---|---:|---:|---:|
| [#41181682430](https://github.com/AkaraChen/fakes3/actions/runs/41181682430) | EPYC 7763 | 5409 → 5294 | 2729 → 1708 | 21.1 s → 10.1 s |
| [#41181684373](https://github.com/AkaraChen/fakes3/actions/runs/41181684373) | EPYC 7763 | 5464 → 5224 | 2597 → 1666 | 20.8 s → 10.1 s |
| [#41181713270](https://github.com/AkaraChen/fakes3/actions/runs/41181713270) | EPYC 7763 | 5026 → 5270 | 2617 → 1725 | 20.3 s → 10.2 s |
| [#41181686197](https://github.com/AkaraChen/fakes3/actions/runs/41181686197) | EPYC 9V74 | 6539 → 7019 | 3156 → 2111 | 21.8 s → 9.6 s |
| [#41181711710](https://github.com/AkaraChen/fakes3/actions/runs/41181711710) | EPYC 9V45 | 9259 → 8668 | 4467 → 2668 | 22.7 s → 9.8 s |

4 KiB GET 不受影响（66～69 vs 68～69 MiB/s）。把 `sendfile` 单次长度限制在 256 KiB / 1 MiB 没有变化。
和 main 的对比也一致（[main #41180082031](https://github.com/AkaraChen/fakes3/actions/runs/41180082031)，multipart 的 `?partNumber=`
读取同样没有收益）。

为什么原型和真实服务不一致没有查清：服务端 CPU 减半是确定的，但 warp 和 fakes3 在同一台 4 核机器上，吞吐被客户端读取
（及它用掉的 CPU）限制；`sendfile` 让客户端读的是冷的 page cache 页而不是刚拷贝过的缓存热数据，单流时客户端成了瓶颈。

## 便宜的改法：没有收益

给普通写路径加单次写入长度上限（256 KiB / 1 MiB），避免一次 `writev` 搬很大一块：
4 路和单流都在噪声内（例如 EPYC 9V45：9259 → 8663 / 9186，单流 4467 → 4484 / 4499；
EPYC 7763：5026 → 5229 / 5349）。hyper 的非阻塞写入本来就受 socket 缓冲区限制，一次最多几 MiB，没有原型里那种整块写的问题。

## 什么时候值得重新考虑

只在「服务端被限制在很少的 CPU 上，而客户端不占同一份 CPU」的部署里 `sendfile` 才可能有净收益（服务端 CPU 少一半）。
测试场景里客户端和服务端通常抢同一批核，并且一次一个 GET 最常见，收益为负。如果以后要做，先在目标部署上用 `exp/sendfile-impl`
（`--no-sendfile` 对照）验证。
