# 多用户（多账户）

fakes3 默认只有一个账户（canonical id / owner 都是 `fakes3`，账户 id `000000000000`），所有带签名的请求都是它。
`--user` 可以再配置若干账户，用来测试跨账户的 S3 语义：ACL 授权给别的账户、Bucket Policy 里指定 Principal、
对象归属（ObjectWriter / BucketOwnerPreferred / BucketOwnerEnforced）、列举结果里的 Owner、ListMultipartUploads 的 Initiator / Owner、
跨账户 CopyObject、`x-amz-expected-bucket-owner`。不配置 `--user` 时行为和性能都和以前一样。

## 1. AWS 怎么建模，fakes3 怎么取舍

AWS 里：

- **账户（account）**有一个 12 位数字的账户 id（`123456789012`），和一个 64 位十六进制的 **canonical user id**
  （`79a59df900b949e55d96a1e698fbacedfd6e09d98eacf8f8d5218e7cd47ef2be`）。S3 的 ACL 用 canonical id 标识被授权人和 owner，
  Bucket Policy 用账户 id / ARN 标识 Principal（也接受 `"CanonicalUser"`）。
  （[AWS account identifiers](https://docs.aws.amazon.com/accounts/latest/reference/manage-acct-identifiers.html)）
- **bucket 和对象都有 owner**，owner 是账户而不是 IAM 用户。bucket owner 是创建者的账户；对象 owner 取决于 bucket 的 Object Ownership 设置：
  `ObjectWriter`（上传者的账户，没有设置 OwnershipControls 的旧 bucket 就是这个行为）、
  `BucketOwnerPreferred`（上传请求带 `bucket-owner-full-control` ACL 时是 bucket owner，否则是上传者）、
  `BucketOwnerEnforced`（永远是 bucket owner，ACL 被禁用）。
  （[Controlling ownership of objects and disabling ACLs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/about-object-ownership.html)）
- **ACL 的被授权人**是 canonical user、`AllUsers`（所有人，含匿名）、`AuthenticatedUsers`（任何 AWS 账户，即任何带有效签名的请求）、`LogDelivery`。
  资源的 owner 总是可以读写该资源的 ACL。
  （[ACL overview](https://docs.aws.amazon.com/AmazonS3/latest/userguide/acl-overview.html)）
- **跨账户授权**要两边都同意：请求方账户内（IAM）允许，加上资源所在账户的 bucket policy / ACL 允许；显式 Deny 总是赢。
  同账户内只要有一边允许就行。bucket owner 不拥有的对象，不能靠 bucket policy 放行（Allow 只对 bucket owner 自己拥有的对象有效），
  要靠对象 ACL，这也是 `bucket-owner-full-control` 存在的原因。
  （[How Amazon S3 authorizes a request](https://docs.aws.amazon.com/AmazonS3/latest/userguide/how-s3-evaluates-access-control.html)、
  [Guidelines for access policy options](https://docs.aws.amazon.com/AmazonS3/latest/userguide/access-policy-alternatives-guidelines.html)）
- Bucket policy 的 Principal：`"*"`、`{"AWS": "arn:aws:iam::<acct>:root"}`（整个账户）、`{"AWS": "<acct>"}`、`{"AWS": "arn:aws:iam::<acct>:user/<name>"}`、
  `{"CanonicalUser": "<canonical id>"}`。
  （[Principal](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_policies_elements_principal.html)）
- `x-amz-expected-bucket-owner` 带的是 bucket owner 的**账户 id**，不符返回 403。
  （[Verifying bucket ownership](https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucket-owner-condition.html)）
- ListBuckets 只返回请求方账户拥有的 bucket；用别人已占用的名字 CreateBucket 返回 409 `BucketAlreadyExists`。

**取舍：一个身份 = 一个账户。** fakes3 没有 IAM，所以没有“账户里的用户”这一层：每个 `--user` 是一个账户的 root 凭证。
S3 的 ACL 和 ownership 语义本来就是账户级别的，这样足够。后果：

- 一个账户一对 access key / secret；账户 id 和 canonical id 在所有身份里唯一。
- 账户内权限是隐含的：账户 root 可以做它自己名下一切（bucket policy 的显式 Deny 除外），不需要 IAM policy。
- Principal `arn:aws:iam::<acct>:user/<name>` 在 `<name>` 是该身份的名字时也匹配（方便把 `--user alice:...` 当成 IAM 用户 alice 来写 policy），
  `...:root` 和裸账户 id 匹配整个账户。没有角色、STS、联合身份。
- 不支持邮箱授权（`AmazonCustomerByEmail` 仍然是 `UnresolvableGrantByEmailAddress`；AWS 对这个也在逐步下线）。
- 没有 RGW 的 tenant（`tenant:bucket`）语法，它不是 S3；s3-tests 的 `tenant` 用户只是第三个账户。

## 2. 接口

```
fakes3 --user alt:ALTKEY:ALTSECRET --user carol:CAROLKEY:CAROLSECRET:111122223333:<64 位十六进制 canonical id>
```

`--user name:access-key:secret[:account-id[:canonical-id]]`，可重复（环境变量 `FAKES3_USERS`，逗号分隔）。

- `--access-key` / `--secret-key` 配置的默认凭证永远是**第 1 个身份**：名字 `fakes3`，账户 id `000000000000`，canonical id `fakes3`。
  它是没法区分身份时的回退身份，也是以前唯一的 owner，所以现有的测试不用改。
- `name` 同时是该账户的 DisplayName（出现在 Owner / Grantee 里）和 policy 里 `user/<name>` 用的名字。
- 省略 `account-id` 时按顺序生成：第 2 个身份是 `000000000001`，第 3 个 `000000000002`……；省略 `canonical-id` 时由账户 id 确定性地生成 64 位十六进制。
  这样同一条命令行每次启动得到相同的 id，测试可以硬编码。
- 校验：access key、账户 id（12 位数字）、canonical id、`name` 在所有身份（含默认的）中唯一；字段不能为空，也不能含引号、逗号和控制字符（除 `name` 外也不能含空白，
  它们会出现在 ACL 的 grant 头和 policy 里）。有问题时启动失败（库里 `Server::start` 返回 `InvalidInput`），错误信息写明原因，不会回显 secret。
- 库：`Config { users: vec![UserConfig { .. }] }`（`UserConfig` 实现 `FromStr`，格式同上）。Node：`start({ users: [{ name, accessKey, secretKey, accountId?, canonicalId? }] })`，
  `server.users` 返回完整列表（包含生成的 id）。
- 控制面 `GET /_fakes3`（`fakes3 ctl status`）列出 `users`：`name`、`access_key`、`account_id`、`canonical_id`（不含 secret）。

### 请求怎么对应到身份

| | 默认（不校验签名） | `--auth` |
|---|---|---|
| 带签名，access key 已配置 | 该身份；**不校验 secret** | 校验签名，用该身份自己的 secret |
| 带签名，access key 未配置 | **第 1 个身份**（以前“任意凭证都是 owner”，保持兼容） | 403 `NotSignedUp` / `SignatureDoesNotMatch` |
| 不带签名 | 匿名 | 匿名 |

“请求是谁”只在 `src/access.rs` 的 `identify` 里决定。Authenticated 的判断也是看有没有签名，所以未配置的 access key 也算 `AuthenticatedUsers`。

## 3. 谁拥有什么

| 资源 | owner |
|---|---|
| bucket | CreateBucket 的请求方 |
| 对象版本 | 见下表；DeleteMarker 的 owner 是执行删除的请求方（bucket 生命周期产生的是 bucket owner） |
| 分段上传 | `Owner` 按对象规则算（CreateMultipartUpload 时确定，Complete 后对象沿用）；`Initiator` 是发起请求的身份 |

对象 owner（PutObject、CopyObject、CreateMultipartUpload、POST 表单上传）：

| bucket 的 OwnershipControls | owner |
|---|---|
| `BucketOwnerEnforced`（`--bucket-defaults aws` 的默认） | bucket owner |
| `BucketOwnerPreferred` | 请求带 `x-amz-acl: bucket-owner-full-control` 时是 bucket owner，否则是上传者 |
| `ObjectWriter`，或没有配置（`legacy` 默认） | 上传者 |

匿名请求（bucket ACL / policy 允许匿名写）上传的对象属于 bucket owner（匿名没有账户）。
owner 随版本保存，并随快照 / 恢复保留；ACL 里的 `bucket-owner-read` / `bucket-owner-full-control` 现在真的会给 bucket owner 加一条授权。

## 4. 授权

请求方 R，bucket owner B，操作需要的 ACL 权限和范围由操作决定（READ = 读对象 / 列举，WRITE = 写入 / 删除，READ_ACP / WRITE_ACP = 读写 ACL）：

1. **bucket policy 里匹配的显式 Deny** → 拒绝（策略管理操作 `Get/Put/DeleteBucketPolicy` 对 B 本人豁免，免得 Deny 一切的策略把 owner 锁死）。
2. 否则以下任一成立就放行：
   - R 是 B，且操作的是 bucket 本身，或对象由 B 拥有（账户 root 隐含权限）；R 是 B 但对象由别人拥有时，只有 ACL / `DeleteObject` 之类的 bucket 级权限能放行——
     bucket owner **读不了**别的账户上传的、没有给它授权的对象；
   - R 是对象 owner（对象 ACL 默认给 owner `FULL_CONTROL`；owner 总能读写自己对象 / bucket 的 ACL）；
   - bucket policy 有匹配 R 的 Allow，且对象（如有）由 B 拥有；
   - ACL 授权给 R 的 canonical id，或授权给 `AuthenticatedUsers` / `AllUsers`（`IgnorePublicAcls` 打开时忽略后两种，`BucketOwnerEnforced` 时整个 ACL 被忽略）。
3. 匿名请求只能靠 policy 的 Allow `*` 或 `AllUsers` ACL，和以前一样；`RestrictPublicBuckets` 同样限制 policy 的公开 Allow 对非 owner 账户的放行。
4. 读一个不存在（或已被删除标记）的对象：能放行读取的请求照常得到 404；没有读权限但有列举权限（bucket owner、bucket ACL 的 READ、或 policy 对 `s3:ListBucket` 的 Allow）的也是 404，其余是 403。
   评估 policy 里的 `s3:prefix` 条件时，把对象 key 当作前缀（RGW 的行为；S3 文档没有说明这种情况下的 `s3:prefix`）。
5. 分段上传的发起者可以继续（UploadPart / UploadPartCopy / Complete）、列出（ListParts）和放弃（Abort）自己的上传。

只配置一个身份时，签名过的请求就是 owner，沿用上面第 2 条的第一项，`check` 在没有 policy 时一次原子读取就返回，和以前一样。

其它规则：

- CreateBucket：名字已被别的账户占用 → 409 `BucketAlreadyExists`；已是自己的 → 200（us-east-1 的旧行为，不变）。
- ListBuckets：只列请求方的 bucket，`Owner` 是请求方。
- GetBucketAcl / GetObjectAcl 的 `Owner` 是 bucket / 对象的 owner；ListObjects(V2) 的 `Owner`、ListObjectVersions 的 `Owner` 是各对象版本的 owner。
- `x-amz-expected-bucket-owner`（以及复制的 `x-amz-source-expected-bucket-owner`）必须等于对应 bucket owner 的账户 id，否则 403 `AccessDenied`。
- CopyObject / UploadPartCopy：请求方对源对象要有读权限，对目标要有写权限；新对象的 owner 按上表，ACL 不随源对象复制。

## 5. 实现与性能

- 身份是 `UserId(u16)`，索引进启动时建好的、之后不变的 `Users` 表。bucket、对象版本、分段上传存的是 `UserId`，不是字符串；列举时 owner 的 XML / DTO 从表里借用，不为每个 key 分配。
  表里每个身份预先渲染好 `<Owner>…</Owner>` 片段（`Users::owner_xml`），给直接写 XML 的列举路径用。
- 单身份时：`identify` 直接返回第 1 个身份（不查表），`check` 的快速路径不变；多身份时才走完整的授权路径，并且只在带 `x-amz-expected-bucket-owner` 头时才比对账户 id。
- 快照 / 恢复把 bucket 的 owner、每个版本的 owner、上传的 initiator / owner 一起复制（`snapshot.rs` 里逐字段解构，新增字段会编译失败）。
- 内存统计只统计对象数据，不受影响。

## 6. 和 ceph/s3-tests 的对应

`conformance/s3-tests/run.sh` 在 fakes3 上给 `[s3 alt]` 和 `[s3 tenant]` 配了各自的凭证和 canonical id
（`--user` 启动参数通过 `FAKES3_USERS` 传给容器），其它服务器保持原样（它们只认识一个凭证）。
`main` 的 `user_id` / `display_name` 仍是 `fakes3`。结果和分类见 [conformance/S3-TESTS.md](../conformance/S3-TESTS.md)。
