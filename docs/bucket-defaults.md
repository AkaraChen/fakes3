# 新建 bucket 的默认值

AWS 在 2023 年改了新 bucket 的默认设置。fakes3 默认**保留旧的行为**（`--bucket-defaults legacy`），这样“先给 bucket 挂一个公开 policy，再匿名读取”之类的测试准备步骤开箱即用；
需要和今天的 AWS 一致时用 `--bucket-defaults aws`（或 `FAKES3_BUCKET_DEFAULTS=aws`、`Config { bucket_defaults: BucketDefaults::Aws, .. }`、Node 的 `start({ bucketDefaults: "aws" })`）。

| | `legacy`（默认） | `aws` |
|---|---|---|
| Object Ownership | 未配置，ACL 可用；`GetBucketOwnershipControls` 返回 `OwnershipControlsNotFoundError` | `BucketOwnerEnforced`，ACL 被禁用；`GetBucketOwnershipControls` 返回 `BucketOwnerEnforced` |
| Block Public Access | 未配置；`GetPublicAccessBlock` 返回 `NoSuchPublicAccessBlockConfiguration` | 四项全开（`BlockPublicAcls`、`IgnorePublicAcls`、`BlockPublicPolicy`、`RestrictPublicBuckets` 均为 `true`） |
| 默认加密（SSE-S3） | 无：`GetBucketEncryption` 返回 `ServerSideEncryptionConfigurationNotFoundError`，PUT / GET / HEAD 不带 `x-amz-server-side-encryption`（显式要求 SSE 时照常） | `GetBucketEncryption` 返回 `AES256`，新对象的 PUT / GET / HEAD 带 `x-amz-server-side-encryption: AES256`；`DeleteBucketEncryption` 之后回到 SSE-S3 |
| 默认禁用 SSE-C | 允许 | 禁用（2026 年 4 月起）：`GetBucketEncryption` 带 `BlockedEncryptionTypes` = `SSE-C`；要求 SSE-C 的 `PutObject`、`CopyObject`、分段上传返回 403 `AccessDenied`，已有的 SSE-C 对象仍可带 key 读取 |

`aws` 模式下具体的行为都有 AWS 文档依据（文档只写明了显式设置 `BucketOwnerEnforced` 时 `CreateBucket` 的错误码；没带 `x-amz-object-ownership` 的默认情况沿用同一个错误码）：

- `CreateBucket` 带 `x-amz-acl`（`public-read` 等）或 `x-amz-grant-*` 头，又没有显式设置 `x-amz-object-ownership` 时失败，
  返回 400 `InvalidBucketAclWithObjectOwnership`，bucket 不会被创建。只给 owner 完全控制的 ACL（`x-amz-acl: private`）可以通过。
  要在创建时设置 ACL，必须同时带 `x-amz-object-ownership: ObjectWriter` 或 `BucketOwnerPreferred`；
  如果 ACL 是公开的，仍然会失败（403 `AccessDenied`，因为 Block Public Access 是开着的）。
- 显式的 `x-amz-object-ownership` 按请求为准（`BucketOwnerEnforced`、`BucketOwnerPreferred`、`ObjectWriter`），Block Public Access 仍然全开。
- ACL 被禁用时：`PutBucketAcl` / `PutObjectAcl` / 带 ACL 头的 `PutObject` 返回 400 `AccessControlListNotSupported`（只接受 `bucket-owner-full-control` 或不带 ACL），读 ACL 永远只显示 owner 的完全控制。
- Block Public Access 全开时：公开的 bucket policy 在 `PutBucketPolicy` 时被拒绝（403 `AccessDenied`），公开的 ACL 被拒绝，匿名请求得不到访问。
- 想要匿名访问，需要明确放开：`DeletePublicAccessBlock`（或 `PutPublicAccessBlock` 关掉相应的项），再用 policy 授权；
  走 ACL 的话还要 `DeleteBucketOwnershipControls` 或 `PutBucketOwnershipControls` 改成 `ObjectWriter` / `BucketOwnerPreferred`。
  这和 AWS 文档里的做法一致。

默认值只在创建 bucket 的那一刻生效。已有的 bucket、快照和 `restore` 都保持各自当前的设置，`reset` 之后新建的 bucket 按当前模式重新开始。
当前模式可以在 `fakes3 ctl status` / `GET /_fakes3` 里看到（`bucket_defaults`）。

- 要在 `aws` 模式的 bucket 里用 SSE-C，先 `PutBucketEncryption`，在规则里写 `BlockedEncryptionTypes` = `NONE`（只有这一项的规则也可以，默认加密保持 SSE-S3）；
  写 `SSE-C` 则再次禁用。没有写 `BlockedEncryptionTypes` 的 `PutBucketEncryption` 按字面处理，即不禁用任何类型（文档没有说明省略时的行为）。
  `DeleteBucketEncryption` 回到默认配置，也就是重新禁用 SSE-C。表单上传（POST policy）目前不处理 SSE 字段，所以没有对应的检查。

`aws` 模式只包含上面有文档依据的默认值，没有按日期划分的“兼容版本”，因为 AWS 没有发布精确的行为变更记录。

来源：

- [S3 Document history](https://docs.aws.amazon.com/AmazonS3/latest/userguide/WhatsNew.html)：2023-04-27 “Amazon S3 now automatically enables S3 Block Public Access and disables S3 access control lists (ACLs) for all new S3 buckets in all AWS Regions”，
  [公告](https://aws.amazon.com/about-aws/whats-new/2023/04/amazon-s3-security-best-practices-buckets-default)
- [Controlling ownership of objects and disabling ACLs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/about-object-ownership.html)、
  [Troubleshooting](https://docs.aws.amazon.com/AmazonS3/latest/userguide/object-ownership-error-responses.html)（`AccessControlListNotSupported`、`InvalidBucketAclWithObjectOwnership`）
- [Blocking public access to your Amazon S3 storage](https://docs.aws.amazon.com/AmazonS3/latest/userguide/access-control-block-public-access.html)
- [CreateBucket API](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CreateBucket.html)：`x-amz-object-ownership` 默认 `BucketOwnerEnforced`；
  “To set an ACL on a bucket as part of a CreateBucket request, you must explicitly set S3 Object Ownership for the bucket to a different value than the default”
- [Default encryption FAQ](https://docs.aws.amazon.com/AmazonS3/latest/userguide/default-encryption-faq.html)、[GetBucketEncryption](https://docs.aws.amazon.com/AmazonS3/latest/API/API_GetBucketEncryption.html)：
  2023-01-05 起所有新对象默认 SSE-S3，响应头 `x-amz-server-side-encryption`
- [Default SSE-C setting for new buckets FAQ](https://docs.aws.amazon.com/AmazonS3/latest/userguide/default-s3-c-encryption-setting-faq.html)、
  [Blocking or unblocking SSE-C](https://docs.aws.amazon.com/AmazonS3/latest/userguide/blocking-unblocking-s3-c-encryption-gpb.html)、
  [BlockedEncryptionTypes](https://docs.aws.amazon.com/AmazonS3/latest/API/API_BlockedEncryptionTypes.html)：2026 年 4 月起新的通用 bucket 默认禁用 SSE-C
