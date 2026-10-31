# 绕开 s3s 的行为差异

fakes3 的协议层（路由、XML、签名校验）由 [s3s](https://github.com/Nugine/s3s) 0.17 负责。s3s 在少数地方和 AWS S3 的表现不一样，
[S3-TESTS.md](../conformance/S3-TESTS.md) 里把它们归为“s3s limitation”。fakes3 不 fork、不 vendor s3s，只在服务外层绕开其中**便宜**的几个。

便宜的标准：

- 只对受影响的请求生效（SigV2 签名的请求、POST 的错误响应、没有长度的上传），SigV4 的常规路径上最多多一次头部查找；
- 代码量小，集中在 [`src/sigv2.rs`](../src/sigv2.rs) 和 [`src/s3s_compat.rs`](../src/s3s_compat.rs)，入口只有 `src/lib.rs` 里的几行；
- 用 A/B 基准确认没有可测量的性能损失。

除了 POST 的错误改写，下面的绕开都只在 `--auth`（校验签名）时运行：默认模式在 s3s 看到请求之前就去掉了签名（见 [`src/noauth.rs`](../src/noauth.rs)），
这些差异在默认模式下不存在。

## 已绕开

### SigV2：Date / x-amz-date 有问题，s3s 回 400，S3 回 403

| | |
|---|---|
| s3s | `x-amz-date` 只按 SigV4 的紧凑 ISO 8601 解析，缺失或解析失败一律是 400 `InvalidRequest`（`missing date`、`invalid x-amz-date`、`invalid date`） |
| S3 | 缺失、空、无法解析、早于 1970 年：403 `AccessDenied`；能解析但和服务器时间差超过 15 分钟：403 `RequestTimeTooSkewed` |
| fakes3 | `sigv2::bad_date` 在请求交给 s3s 之前检查头部（`x-amz-date` 优先，否则 `Date`；接受 RFC 1123 和紧凑 ISO 8601），不合格就直接回 S3 的答案。只对 `Authorization: AWS ...` 的请求运行 |
| 为什么这样做 | 只改写错误响应做不到：400 的消息区分不了“1950 年”和“9999 年”。检查本身是一次头部解析，只在 SigV2 请求上发生 |
| 没解决 | 格式正确的 RFC 1123 `x-amz-date`（S3 接受）仍然被 s3s 拒绝（400）。要接受它得自己验证 SigV2 签名，不便宜。常见 SDK（botocore、Java、Go）发的是 `Date`，不受影响 |
| 何时可以删 | s3s 的 `v2_check_header_auth` 返回 `AccessDenied`，并接受 RFC 1123 的 `x-amz-date` |

测试：`v2_bad_dates_are_access_denied`（`tests/s3s_workarounds.rs`）。

### SigV2：botocore 对 bucket 级请求签 `/bucket/`，s3s 签 `/bucket`

| | |
|---|---|
| s3s | StringToSign 的 CanonicalizedResource 直接用请求路径，`/bucket` 就是 `/bucket` |
| S3 / botocore | botocore 对请求行是 `/bucket` 的 path-style 请求签 `/bucket/`（`CreateBucket`、`ListObjects`、`HeadBucket`……）；S3 两种都接受。所以 boto3 的 SigV2 bucket 级调用在 s3s 上全部 `SignatureDoesNotMatch` |
| fakes3 | SigV2 签名的 bucket 级请求（路径只有一段）先原样交给 s3s；如果回 403 `SignatureDoesNotMatch`，把路径改成 `/bucket/` 再调用一次。HEAD 的响应没有 body，所以 HEAD 在任何 403 后重试，并且只有第二次不是 403 才采用第二次的结果 |
| 安全性 | 验签仍然由 s3s 完成，用的还是配置的密钥；`/bucket` 和 `/bucket/` 是同一个操作，接受任一种写法不会放宽授权。第一次失败发生在操作执行之前，重放不会重复副作用 |
| 代价 | 只有 SigV2 的 bucket 级请求会把 body 读进内存（上限 4 MiB，超过或没有 Content-Length 的不重试）。SigV4 请求只多一次 `Authorization` 头查找 |
| 何时可以删 | s3s 的 `create_string_to_sign` 对 bucket 级路径两种写法都试 |

测试：`v2_bucket_level_requests_verify_with_or_without_the_slash`（含预签名 URL、带 body 的 DeleteObjects、错误签名仍被拒绝）。

### POST policy 条件不满足：s3s 回 400 `InvalidPolicyDocument`，S3 回 403 `AccessDenied`

| | |
|---|---|
| s3s | 表单字段不满足 policy 里的 `eq` / `starts-with` 条件、`bucket` 条件不符、`x-amz-date` / `x-amz-credential` / `x-amz-algorithm` 和 policy 不符，都是 400 `InvalidPolicyDocument` |
| S3 | 403 `AccessDenied: Invalid according to Policy: ...` |
| fakes3 | POST 表单带签名字段（`signature` 或 `x-amz-signature`）、响应是 400 且消息是上述几种之一时，把响应改写成 403 `AccessDenied`（消息前加 `Invalid according to Policy: `）。无法解析的 policy 仍然是 400 `InvalidPolicyDocument`（S3 也是），`content-length-range` 的 `EntityTooLarge` / `EntityTooSmall` 不动 |
| 为什么只改带签名的表单 | s3s 把没有签名字段的表单当匿名请求，仍然先检查 policy；S3 先因缺少签名回 400。不区分的话，缺签名的表单会从 400 变成 403（`test_post_object_missing_signature`） |
| 代价 | 只有 `multipart/form-data` 的 POST、且 body 不超过 8 MiB 才会被读进内存找签名字段（更大的表单不改写，保持 400）；改写本身只发生在 400 响应上，成功路径没有额外工作。两种认证模式都生效 |
| 何时可以删 | s3s 的 `post_policy.rs` 把条件不满足报成 `AccessDenied` |

测试：`post_policy_violations_are_access_denied`（SigV2 签名的表单，`--auth` 和默认模式各一遍）。

### SigV4：只有 `Date`、没有 `x-amz-date`，s3s 回 `missing header: x-amz-date`

| | |
|---|---|
| s3s | SigV4 的请求时间只从 `x-amz-date` 取，缺失就是 400 `InvalidRequest` |
| S3 | `x-amz-date` 或 `Date` 任一个都行。botocore 在请求里有 `Date` 头时会删掉 `x-amz-date`，用 `Date`（RFC 1123）发请求，签名里的时间仍是紧凑格式 |
| fakes3 | 请求带 `Authorization`、有 `Date`、没有 `x-amz-date` 时，按 `Date` 补一个紧凑格式的 `x-amz-date`。签名通过 StringToSign 绑定了这个时间，编造的值验证不过。s3s 还要求 `x-amz-*` 头都在 SignedHeaders 里，所以配置里把 `x-amz-date` 加进了 s3s 的 `unsigned_amz_header_allowlist`（同样安全：它被签名覆盖） |
| 代价 | 每个 SigV4 请求多一次 `x-amz-date` 头查找（存在就立刻返回） |
| 何时可以删 | s3s 在没有 `x-amz-date` 时回退到 `Date` |

S3-TESTS.md 之前把这两个测试归因于“RFC 1123 的 `X-Amz-Date`”；实际上请求里没有 `X-Amz-Date`，只有 `Date`。

测试：`sigv4_takes_the_time_from_date` 和 `s3s_compat` 的单元测试。

### `Transfer-Encoding: chunked` 且没有 `Content-Length` 的 PutObject

| | |
|---|---|
| s3s | 校验 SigV4 时要求 `Content-Length`（或能从 body 得知长度），否则 411 `MissingContentLength`。默认模式（不校验签名）不受影响 |
| S3 | 接受 |
| fakes3 | `--auth` 下，带 `Transfer-Encoding` 又没有 `Content-Length` 的请求（`aws-chunked` 流式上传除外，它们有 `x-amz-decoded-content-length`）先把 body 读进内存，设上 `Content-Length` 再交给 s3s；s3s 随后照常校验 `x-amz-content-sha256` |
| 代价 | 这类请求要整个 body 缓冲，上限 64 MiB，超过按原来的方式回 411。其他请求只多一次 `Transfer-Encoding` 头查找 |
| 何时可以删 | s3s 接受没有长度的单块上传 |

测试：`uploads_without_content_length`（两种模式）。

## 没有绕开

目前没有因为“太贵”而放弃的 s3s 差异：S3-TESTS.md 里“s3s limitation”一节的 25 个测试，上面的绕开覆盖了对应的原因。
没有解决的只有上面“SigV2：Date / x-amz-date”一节说的格式正确的 RFC 1123 `x-amz-date`，没有测试覆盖它；
真要支持就得在 fakes3 里重新实现 SigV2 验签，这违反了“不 fork s3s、不重写它的逻辑”的约束。

## 之前就有的绕开

- 默认模式去掉签名、解码 aws-chunked：[`src/noauth.rs`](../src/noauth.rs)
- 预签名 URL 按服务器时钟判断过期：[`src/presign.rs`](../src/presign.rs)
- HEAD `?partNumber=` 回 206、重复头部合并、OPTIONS 预检：`src/lib.rs`
