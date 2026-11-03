# TODO

## 测试控制面（`/_fakes3/*`，见 [docs/control-plane.md](docs/control-plane.md)）

- [x] `POST /_fakes3/reset`：清空所有 bucket / object / upload，同时清空请求日志和时钟
- [x] 快照：`POST /_fakes3/snapshots`、`POST /_fakes3/snapshots/{name}/restore`（索引复制方案，选型和实测数据见文档）
- [x] `/_fakes3/clock`：冻结、推进或设置虚拟时间；预签名 URL 过期跟随虚拟时钟
  - s3s 不允许注入时间，SigV4 日期偏差检查仍然用主机时间（见文档）
  - lifecycle 过期和转换、Object Lock retention 到期、Glacier restore 的完成与到期都读 `Clock::now`
- [ ] `POST /_fakes3/faults`：按操作、bucket/key 前缀或概率注入故障
  - 503 SlowDown、500 InternalError、固定或随机延迟、body 截断、连接重置
- [x] `GET /_fakes3/requests`：请求日志，用于断言调用次数和顺序
- [x] 命令行 `fakes3 ctl` 和 Rust 进程内的 `Server` 方法
- [x] Rust 进程内库模式（`Server::start` / `Server::start_blocking`）、Node.js 绑定（napi-rs）、GHCR 镜像
- [x] testcontainers：不需要专门的模块，通用容器 + 日志等待即可（见 README）
- [x] GitHub Action：[setup-fakes3](https://github.com/AkaraChen/setup-fakes3)，下载 release 里的静态二进制并在后台启动
- [x] OpenDAL 绑定：crate `fakes3-opendal`（[bindings/opendal](bindings/opendal/README.md)），返回走 OpenDAL S3 服务的 `Operator`；
      为此 `Server` 加了 `create_bucket` / `enable_versioning`
- [ ] 其余语言的框架集成（pytest fixture 等，基于 `fakes3 ctl` 或 HTTP API）
- [x] 默认不校验签名（原 `--no-auth`，现在是默认行为，`--auth` 才严格校验）：任意 access key / secret 签名的请求都接受（SigV4 / SigV2 的头和 presigned URL、三种流式上传，trailer 校验和仍会校验），
      并且是 bucket 的 owner；不带签名的请求是匿名请求，照常走 ACL / Bucket Policy / Block Public Access。
      s3s 没有跳过验签的开关，所以在服务包装层去掉签名再交给 s3s（aws-chunked 自己解码），并在请求扩展里留下 `Signed { access_key, presigned }`（见 `src/noauth.rs`、`src/access.rs`）
  - 带签名的 POST 表单上传（`x-amz-signature` / `signature` 字段）仍由 s3s 按配置的凭证校验，任意凭证的签名表单不被接受
  - `--no-auth` 保留为已弃用、不起作用的别名，之后可以删除
- [x] 多个身份：`--user name:access-key:secret[:account-id[:canonical-id]]`（Node 的 `users`，库的 `Config::users`，见 [docs/multi-user.md](docs/multi-user.md)）。
      一个身份就是一个账户；默认模式下按请求里的 access key 选账户、不校验 secret（没配置的 key 是默认账户），`--auth` 下校验该账户的 secret；
      bucket、对象版本、分段上传各存 owner，ACL grantee、Bucket Policy 的 Principal、`x-amz-expected-bucket-owner`、列举里的 Owner 都按账户区分；控制面 `status` 列出账户
  - 不做：IAM 用户和策略、角色与 STS、邮箱授权、RGW 的 tenant 语法

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
- [x] HeadObject 的 `response-*` 覆盖参数（和 GetObject 一样；两者都支持 `response-expires`）
- [x] DeleteObjects 里 `<Object>` 的 `<ETag>` 条件：不满足的那一项单独报 `PreconditionFailed` / `NoSuchKey`，其余照删
- [x] `encoding-type=url`
- [x] 条件请求：If-Match / If-None-Match / If-Modified-Since / If-Unmodified-Since，CopySource 的条件头
- [x] CopyObject 对目标对象的 If-Match / If-None-Match（和 PutObject 一样在写锁内检查）
- [x] 追加写：PutObject 的 `x-amz-write-offset-bytes`。AWS 只在 S3 Express 目录 bucket 支持，fakes3 所有 bucket 都支持；
      offset 必须等于当前大小（`InvalidWriteOffset`），每次追加算一个 part（上限 10000，`TooManyParts`），
      ETag 是 `<摘要>-<part 数>`，FULL_OBJECT 的 CRC 会接着算，SHA 和 composite 校验和接不上就去掉；
      保留原对象的元数据、标签、ACL、存储类型和加密方式，SSE-C 的 key 不一致返回 `EncryptionTypeMismatch`
- [x] 对象和 bucket 的 Tagging
- [x] ACL（canned / grant / 请求体）、OwnershipControls、PublicAccessBlock、Bucket Policy 求值（匿名访问 + 显式 Deny）、
      CORS 预检与响应头、Lifecycle（按服务端时钟惰性执行）、Website 与 Notification 的配置读写
- [ ] Website 端点（按 index/error document 和 routing rules 提供静态网站）、Notification 实际投递
- [x] 新建 bucket 的默认值可切换：默认 `legacy`（ACL 可用、无 Block Public Access、无默认加密、允许 SSE-C），`--bucket-defaults aws` 采用现在的 AWS 默认值
      （BucketOwnerEnforced、Block Public Access 全开、默认 SSE-S3、默认禁用 SSE-C，见 [docs/bucket-defaults.md](docs/bucket-defaults.md) 里的 AWS 文档来源，
      含 [SSE-C FAQ](https://docs.aws.amazon.com/AmazonS3/latest/userguide/default-s3-c-encryption-setting-faq.html)）
- [x] SSE-C（校验 key MD5）、SSE-S3/KMS header 回显、Put/Get/DeleteBucketEncryption 及默认加密
- [x] POST policy 表单上传（条件校验由 s3s 完成；过期时间按宿主机时钟判断，不受虚拟时钟影响）
- [x] virtual-host 寻址：`--domain`（可重复），默认 `localhost`，即 `http://<bucket>.localhost:<port>`
- [x] ListMultipartUploads
- [x] GetObjectAttributes
- [x] 回显 `storage_class`、`x-amz-website-redirect-location` 等剩余元数据
- [x] Glacier / Deep Archive / Intelligent-Tiering 归档层：读返回 `InvalidObjectState`，RestoreObject（202 / 200 / 409、`x-amz-restore`、ListObjectsV2 的 `RestoreStatus`），
      完成和到期按服务端时钟，见 [docs/archive-storage.md](docs/archive-storage.md)
- [x] Lifecycle 的 Transition / NoncurrentVersionTransition（瀑布模型、128 KB 默认下限、Object Lock 不阻止转换）
- [ ] 归档相关的剩余差异：ListObjectVersions 不返回 `RestoreStatus`；SELECT 类型的 RestoreObject 返回 501；不模拟 `GlacierExpeditedRetrievalNotAvailable`；
      Intelligent-Tiering 不记录读取，访问不会重置闲置天数；不校验 Lifecycle 里 Days 的先后和最小值要求

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

- [x] 接入 ceph/s3-tests，作为进度基线（`conformance/s3-tests/run.sh`、`.github/workflows/s3-tests.yml`，分类后的基线和待办见 [conformance/S3-TESTS.md](conformance/S3-TESTS.md)）
- [x] ceph/s3-tests 里“便宜”的一批修复（请求 id、对已有 bucket 再次 CreateBucket 返回 200、`?partNumber=` 的分段校验和、ListObjects 字段、Object Lock 边角、Content-MD5、
      `x-amz-expected-bucket-owner` 等，见 [conformance/S3-TESTS.md](conformance/S3-TESTS.md)）：AWS 适用的 816 个里通过 475 → 513
  - 剩下的 `test_copy_part_enc[…-sse-c-…]`（SSE-C 分段上传的 Complete 不带 key 是否应该 400，AWS 行为未确认）、`test_object_read_unreadable`（C1 控制字符的 key）没动
- [x] 接入 Apache OpenDAL 的 behavior tests（`conformance/opendal/run.sh`，结果见 [conformance/OPENDAL.md](conformance/OPENDAL.md)）：
      四种配置全部通过，不用关闭任何能力（v0.1.1 时 12 个失败，#7、#8 补齐）
- [ ] 接入 MinIO mint
- [ ] 对真实 AWS S3 做差分测试（录制和回放）
