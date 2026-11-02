# 认证模式

默认**不校验签名**（LocalStack / moto 式的“任意凭证都可以”），但授权照常生效；`--auth` 才严格校验签名。

| | 默认 | `--auth` |
|---|---|---|
| 配置 | `Config { auth: false, .. }`（Node：`auth: false`） | `--auth` 或 `FAKES3_AUTH=1`；`Config { auth: true, .. }`；Node：`auth: true` |
| 带签名的请求 | 不验签，任意 access key 和 secret 都接受；access key 属于某个 `--user` 时是那个账户，否则是默认账户（`fakes3`，没配置多用户时它就是 owner） | 用配置的 access key / secret（默认凭证和每个 `--user` 各自的）校验 SigV4 / SigV2，错误返回 403（`SignatureDoesNotMatch`、`NotSignedUp`） |
| 不带签名的请求 | **匿名**，走正常的匿名授权：ACL、Bucket Policy、Block Public Access、OwnershipControls | 同左 |
| 授权（含显式 Deny） | 生效 | 生效 |

也就是说，两种模式下授权的行为完全一样，差别只在“带签名”是否要验证：默认模式下，任何带了签名的请求都是 owner，
所以 `PutBucketPolicy`、`PutBucketAcl` 等 owner 才能做的操作用任意凭证即可；而不带签名的请求在私有 bucket 上会得到 403 `AccessDenied`，
要在 policy 或公开 ACL 允许之后才能访问。需要“请求没有签名”时，用不带 `Authorization` 头、也不是 presigned URL 的请求就行
（比如 `curl`、浏览器直接访问）。

默认模式覆盖 SigV4 的 `Authorization` 头和 presigned URL、SigV2 的头和 presigned URL，以及流式上传
（`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`、`STREAMING-UNSIGNED-PAYLOAD-TRAILER`、`STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`）。几点说明：

- **完整性校验仍然生效**：aws-chunked 的分块照常解码，trailer 里的校验和（`x-amz-checksum-*`）仍会和数据比对，不一致返回 400 `BadDigest`；
  `Content-MD5`、header 里的校验和、`x-amz-decoded-content-length` 也一样。不再检查的是签名本身以及由它派生的东西：
  时钟偏差、`x-amz-content-sha256` 里的负载哈希、signed headers 列表。
- **presigned URL 的过期**：只在使用虚拟时钟（`/_fakes3/clock`）时判断，真实时钟下不检查。
- 请求日志的 `access_key` 字段在默认模式下记录请求用的 access key（未签名的请求为 `null`）；`--auth` 时它永远是配置的那一个，所以不记录。
- Bucket Policy 的 `aws:PrincipalType`、`s3:authType` 等条件对带签名的请求按 owner 求值，presigned URL 的 `s3:authType` 是 `REST-QUERY-STRING`。
- 带签名的 POST 表单上传（表单里有 `x-amz-signature` / `signature` 字段）仍由 s3s 校验，所以必须用配置的 access key / secret 签名（任意凭证不行）；不带签名的表单是匿名请求。
- **从旧版本升级**：以前默认校验签名，现在不校验；要保持原样，加 `--auth`（Node：`auth: true`，库：`Config { auth: true, .. }`）。
  `--no-auth` 以前还会跳过全部授权，现在它只是一个已弃用、不起作用的别名（默认就不校验签名），启动时会在 stderr 提示；
  依赖“未签名请求等同 owner”的测试需要改为带上任意签名（例如任何 SDK 的随机凭证）。
- 多个账户用 `--user` 配置（见[多用户](multi-user.md)）：默认模式下按请求里的 access key 选账户、不校验 secret，`--auth` 下校验该账户自己的 secret；
  “带签名的请求是谁”的判断集中在 `src/access.rs` 的 `identify`。
