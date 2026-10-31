# TODO

## 测试控制面（`/_fakes3/*`，见 [docs/control-plane.md](docs/control-plane.md)）

- [x] `POST /_fakes3/reset`：清空所有 bucket / object / upload，同时清空请求日志和时钟
- [x] 快照：`POST /_fakes3/snapshots`、`POST /_fakes3/snapshots/{name}/restore`（索引复制方案，选型和实测数据见文档）
- [x] `/_fakes3/clock`：冻结、推进或设置虚拟时间；预签名 URL 过期跟随虚拟时钟
  - s3s 不允许注入时间，SigV4 日期偏差检查仍然用主机时间（见文档）
  - 以后的 lifecycle 过期、Object Lock retention 到期、Glacier restore 完成应当读 `Clock::now`
- [ ] `POST /_fakes3/faults`：按操作、bucket/key 前缀或概率注入故障
  - 503 SlowDown、500 InternalError、固定或随机延迟、body 截断、连接重置
- [x] `GET /_fakes3/requests`：请求日志，用于断言调用次数和顺序
- [x] 命令行 `fakes3 ctl` 和 Rust 进程内的 `Server` 方法
- [x] Rust 进程内库模式（`Server::start` / `Server::start_blocking`）、Node.js 绑定（napi-rs）、GHCR 镜像
- [x] testcontainers：不需要专门的模块，通用容器 + 日志等待即可（见 README）
- [ ] 其余语言的框架集成（pytest fixture 等，基于 `fakes3 ctl` 或 HTTP API）
- [ ] `--no-auth` 模式下已签名的请求会得到 501 `This service has no authentication provider`（s3s 在没有 auth provider 时拒绝带签名的请求），只有匿名请求能通过

## S3 语义完整度

- [x] 版本控制：VersionId、删除标记、null version、Suspended、ListObjectVersions
- [x] 版本控制补全：MFA Delete、版本号格式校验、ListObjectVersions 的 marker/max-keys 边角；
      仍有差异：`<Version>` 和 `<DeleteMarker>` 在 XML 里是分开成组的（s3s 的限制），AWS 是按 key 交错的
- [x] Object Lock：retention、legal hold、governance bypass、默认保留期，到期按服务端时钟判断
- [x] 分段上传对象的 composite 校验和（CRC32 / CRC32C / SHA1 / SHA256）
- [x] 分段上传的 FULL_OBJECT 校验和（CRC32 / CRC32C / CRC64NVME，用 crc-fast 合并各 part 的 CRC）
- [x] 未指定算法时默认计算 CRC64NVME（与 2024 年 12 月之后的 S3 一致）
- [x] UploadPartCopy（零拷贝，共享源对象的 buffer）
- [x] GET/HEAD `?partNumber=`、HEAD 的 Range
- [x] `encoding-type=url`
- [x] 条件请求：If-Match / If-None-Match / If-Modified-Since / If-Unmodified-Since，CopySource 的条件头
- [x] 对象和 bucket 的 Tagging
- [x] ACL（canned / grant / 请求体）、OwnershipControls、PublicAccessBlock、Bucket Policy 求值（匿名访问 + 显式 Deny）、
      CORS 预检与响应头、Lifecycle（按服务端时钟惰性执行）、Website 与 Notification 的配置读写
- [ ] Website 端点（按 index/error document 和 routing rules 提供静态网站）、Notification 实际投递、
      Lifecycle 的 Transition
- [x] 新建 bucket 的默认值可切换：默认 `legacy`（ACL 可用、无 Block Public Access、无默认加密），`--bucket-defaults aws` 采用现在的 AWS 默认值
      （BucketOwnerEnforced、Block Public Access 全开、默认 SSE-S3，见 [README](README.md#新建-bucket-的默认值)）
- [ ] `aws` 默认值里还差一项：新 bucket 默认禁用 SSE-C（2026-04，`BlockedEncryptionTypes`），待确认是否实现
- [x] SSE-C（校验 key MD5）、SSE-S3/KMS header 回显、Put/Get/DeleteBucketEncryption 及默认加密
- [x] POST policy 表单上传（条件校验由 s3s 完成；过期时间按宿主机时钟判断，不受虚拟时钟影响）
- [x] virtual-host 寻址：`--domain`（可重复），默认 `localhost`，即 `http://<bucket>.localhost:<port>`
- [x] ListMultipartUploads
- [x] GetObjectAttributes
- [x] 回显 `storage_class`、`x-amz-website-redirect-location` 等剩余元数据
- [ ] Glacier / Deep Archive 对象的 `InvalidObjectState` 和 RestoreObject（目前这两类对象仍可直接读取）

## 性能

- [x] 评估明文 HTTP 下大对象 GET 走 `sendfile`：服务端 CPU 减半，但端到端吞吐没有提升，单流还慢约 37%，结论不做，见 [bench/SENDFILE.md](bench/SENDFILE.md)（实现在分支 `exp/sendfile-impl`）
- [x] MD5 优化：换成 OpenSSL 的汇编实现（通过底层 `MD5_*` 接口调用，不链接 EVP，二进制体积不变），
      大于 inline 阈值的请求体把 MD5 放到独立线程，与接收、校验和、写 memfd 流水线并行
- [ ] 多个并发流共享 SIMD 的多路 MD5（类似 MinIO 的 md5-simd），提升多并发大对象写入的总吞吐
- [x] 内存上限（`--memory-limit`）和超出上限时的明确报错（507 `XFakeS3MemoryLimitExceeded`）
- [x] 评估每 bucket 的 `RwLock<BTreeMap>` 是否是瓶颈：CI（4 核）上 64 并发 PUT 与 16 并发吞吐持平（约 2.3 万 ops/s），
      服务端只用了约 0.74 核，瓶颈在压测客户端而不是锁，暂不更换。
- [x] 快照没有换成 `im::OrdMap`：实测持久化 map 让每次写入的“构造并插入”慢 2.2～2.6 倍（HTTP 层面吞吐低约 1.5%），而复制 `BTreeMap` 在 10 万个对象时只要几毫秒（数据见
      [docs/control-plane.md](docs/control-plane.md#快照的实现和选型)）；需要时只改 `store::Index` 一处

## 一致性验证

- [ ] 接入 ceph/s3-tests，作为进度基线
- [ ] 接入 MinIO mint
- [ ] 对真实 AWS S3 做差分测试（录制和回放）
