# TODO

## 测试控制面（`/_fakes3/*`，后续计划）

- [ ] `POST /_fakes3/reset`：清空所有 bucket / object / upload，微秒级完成
- [ ] `POST /_fakes3/snapshot` → `{id}`；`POST /_fakes3/restore/{id}`：O(1) 快照和回滚
  - 前提：元数据换成持久化数据结构（`im::OrdMap`），Blob 本身已经不可变且有引用计数
- [ ] `POST /_fakes3/clock`：冻结、推进或设置虚拟时间
  - 驱动 presigned URL 过期、lifecycle 过期、Object Lock retention 到期、Glacier restore 完成
- [ ] `POST /_fakes3/faults`：按操作、bucket/key 前缀或概率注入故障
  - 503 SlowDown、500 InternalError、固定或随机延迟、body 截断、连接重置
- [ ] `GET /_fakes3/requests`：请求日志，用于断言调用次数和顺序
- [ ] 各语言客户端封装（Rust 进程内库模式、testcontainers 模块、pytest fixture、Go helper、JUnit extension）

## S3 语义完整度

- [x] 版本控制：VersionId、删除标记、null version、Suspended、ListObjectVersions
- [ ] 版本控制补全：删除标记响应头以外的边角（MFA delete、ListObjectVersions 与 AWS 的逐字节差分）
- [ ] Object Lock：retention、legal hold、governance bypass
- [x] 分段上传对象的 composite 校验和（CRC32 / CRC32C / SHA1 / SHA256）
- [ ] 分段上传的 FULL_OBJECT 校验和（需要 CRC combine）
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
- [ ] 单流 PUT 吞吐的瓶颈在 MD5（约 460 MB/s）。md5-asm 实测没有提升；openssl 的汇编实现快约 25%；
      另一个方向是把 MD5 计算放到独立线程，与接收数据流水线并行
- [ ] 内存上限和超出上限时的明确报错

## 一致性验证

- [ ] 接入 ceph/s3-tests，作为进度基线
- [ ] 接入 MinIO mint
- [ ] 对真实 AWS S3 做差分测试（录制和回放）
