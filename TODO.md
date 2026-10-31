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
- [x] 命令行 `fakes3 ctl` 和 Rust 进程内的 `Server` 方法（不做各语言的客户端封装）
- [ ] testcontainers 模块、pytest fixture 之类的框架集成（基于 `fakes3 ctl` 或 HTTP API）

## S3 语义完整度

- [x] 版本控制：VersionId、删除标记、null version、Suspended、ListObjectVersions
- [ ] 版本控制补全：删除标记响应头以外的边角（MFA delete、ListObjectVersions 与 AWS 的逐字节差分）
- [ ] Object Lock：retention、legal hold、governance bypass
- [x] 分段上传对象的 composite 校验和（CRC32 / CRC32C / SHA1 / SHA256）
- [x] 分段上传的 FULL_OBJECT 校验和（CRC32 / CRC32C / CRC64NVME，用 crc-fast 合并各 part 的 CRC）
- [x] 未指定算法时默认计算 CRC64NVME（与 2024 年 12 月之后的 S3 一致）
- [x] UploadPartCopy（零拷贝，共享源对象的 buffer）
- [x] GET/HEAD `?partNumber=`、HEAD 的 Range
- [x] `encoding-type=url`
- [x] 条件请求：If-Match / If-None-Match / If-Modified-Since / If-Unmodified-Since，CopySource 的条件头
- [x] 对象和 bucket 的 Tagging
- [ ] ACL、Bucket Policy 求值、CORS 预检、Lifecycle、Website、Notification
- [ ] SSE-C（校验 key MD5）、SSE-S3/KMS header 回显
- [ ] POST policy 表单上传
- [ ] virtual-host 寻址的默认域名配置
- [x] ListMultipartUploads
- [ ] GetObjectAttributes
- [ ] 回显 `storage_class`、`x-amz-website-redirect-location` 等剩余元数据

## 性能

- [ ] 明文 HTTP 下大对象 GET 走 `sendfile`/`splice`（需要绕过 s3s 的 body 抽象）
- [x] MD5 优化：换成 OpenSSL 的汇编实现（通过底层 `MD5_*` 接口调用，不链接 EVP，二进制体积不变），
      大于 inline 阈值的请求体把 MD5 放到独立线程，与接收、校验和、写 memfd 流水线并行
- [ ] 多个并发流共享 SIMD 的多路 MD5（类似 MinIO 的 md5-simd），提升多并发大对象写入的总吞吐
- [x] 内存上限（`--memory-limit`）和超出上限时的明确报错（507 `XFakeS3MemoryLimitExceeded`）
- [x] 评估每 bucket 的 `RwLock<BTreeMap>` 是否是瓶颈：CI（4 核）上 64 并发 PUT 与 16 并发吞吐持平（约 2.3 万 ops/s），
      服务端只用了约 0.74 核，瓶颈在压测客户端而不是锁，暂不更换。
- [ ] 做测试控制面的快照功能时，把索引换成 `im::OrdMap`（O(1) 快照）；届时在核数更多的机器上复测写入争用

## 一致性验证

- [ ] 接入 ceph/s3-tests，作为进度基线
- [ ] 接入 MinIO mint
- [ ] 对真实 AWS S3 做差分测试（录制和回放）
