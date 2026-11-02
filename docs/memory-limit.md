# 内存上限

fakes3 统计所有对象数据实际占用的内存（堆上的小对象、memfd 里的大对象、尚未 Complete 的分段上传的 part），
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
