# 归档存储类

用来测试“应用遇到已归档对象时怎么办”。对象带上 `GLACIER`（S3 Glacier Flexible Retrieval）或 `DEEP_ARCHIVE` 存储类写入（或被 Lifecycle 转过去）之后：

- **读被拒绝**：GET（包括 Range、`?partNumber=`）、CopyObject 和 UploadPartCopy 的来源返回 `403 InvalidObjectState`，
  直到有一份已恢复的副本；HEAD、标签、GetObjectAttributes 照常工作，HEAD 返回真实存储类。
  `GLACIER_IR`（Instant Retrieval）和 `STANDARD_IA` 等不是归档，可以直接读。
- **RestoreObject**：`Days`（必填）加 `Tier`（`Expedited` / `Standard` / `Bulk`，可放在 `GlacierJobParameters` 里也可放在顶层，默认 Standard）。
  新请求返回 `202`，已恢复的对象返回 `200` 并从*现在*起重新计算保留天数，进行中返回 `409 RestoreAlreadyInProgress`
  （发更快的 tier 算加速，返回 `202`）；对不是归档的对象返回 `403 ObjectAlreadyInActiveTierError`；
  `Type=SELECT`、`SelectParameters`、`OutputLocation` 返回 `501 NotImplemented`（S3 Select 已不对新客户开放）；
  Deep Archive 上的 `Expedited` 返回 `400 InvalidArgument`；不模拟 503 `GlacierExpeditedRetrievalNotAvailable`。支持 `?versionId=`。
- **`x-amz-restore`**（GET / HEAD）：进行中是 `ongoing-request="true"`，完成后是 `ongoing-request="false", expiry-date="Sat, 05 Jan 2030 00:00:00 GMT"`；
  没有恢复过则没有这个头。恢复出来的副本到期后对象重新变回不可读。ListObjectsV2 带 `x-amz-optional-object-attributes: RestoreStatus` 时返回 `RestoreStatus`
  （`IsRestoreInProgress` 和 `RestoreExpiryDate`，没恢复过的对象没有）；ListObjectVersions 暂未返回。
- **完成时间按服务端时钟，取 AWS 文档给出的范围的下界，所以是确定的**（`fakes3 ctl clock advance 3h` 就能让 Standard 恢复完成）：

  | | Expedited | Standard | Bulk |
  |---|---|---|---|
  | `GLACIER`、IT Archive Access | 1 分钟 | 3 小时 | 5 小时 |
  | `DEEP_ARCHIVE`、IT Deep Archive Access | 不支持 | 12 小时 | 48 小时 |

  恢复副本的到期时间 = 完成时间 + `Days`，向上取整到 UTC 午夜（文档例子：10 月 15 日 10:30 恢复 3 天，10 月 19 日 00:00 到期）。
- **Intelligent-Tiering 的归档层**：`PutBucketIntelligentTieringConfiguration`（及 Get / List / Delete）配置 `ARCHIVE_ACCESS`（≥ 90 天）和
  `DEEP_ARCHIVE_ACCESS`（≥ 180 天，最长 730 天），可带 Prefix / Tag / And 过滤。`INTELLIGENT_TIERING` 对象从创建（或 Lifecycle 转入、上次恢复完成）起闲置达到天数，
  就进入对应归档层：读返回 `InvalidObjectState`，HEAD 带 `x-amz-archive-status`。恢复不接受 `Days`（传了也忽略），完成后回到 Frequent Access（没有到期时间），闲置天数重新计数。
  偏差：fakes3 不记录读取，所以“访问”不会重置闲置天数；错误体里没有 `StorageClass` / `AccessTier` 元素。
- **Lifecycle 转换**：`Transition`（`Days` / `Date`）和 `NoncurrentVersionTransition`（`NoncurrentDays`、`NewerNoncurrentVersions`）在每天第一次访问 bucket 时的惰性清扫里执行，
  到期时间同样向上取整到 UTC 午夜。只改存储类，数据、ETag、`Last-Modified` 不变，内存统计不变。遵循 S3 的瀑布模型（只能向下转：
  STANDARD → STANDARD_IA → INTELLIGENT_TIERING → ONEZONE_IA → GLACIER_IR → GLACIER → DEEP_ARCHIVE），时钟一次拨得很远时直接落到最深的那个到期类；
  小于 128 KB 的对象默认不转换，除非规则带 `ObjectSizeGreaterThan` / `ObjectSizeLessThan`，或者 `x-amz-transition-default-minimum-object-size: varies_by_storage_class`
  （此时小对象只能转到 `GLACIER` / `DEEP_ARCHIVE`）。Object Lock 不阻止转换，转换后保留期照旧。
  转换目标只能是 `STANDARD_IA`、`INTELLIGENT_TIERING`、`ONEZONE_IA`、`GLACIER_IR`、`GLACIER`、`DEEP_ARCHIVE`。
  没有校验 AWS 对 Days 的大小关系要求（例如转到 IA 至少 30 天、过期天数要大于转换天数）。

AWS 文档：[RestoreObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_RestoreObject.html)、
[GetObject 的 InvalidObjectState](https://docs.aws.amazon.com/AmazonS3/latest/API/API_GetObject.html)、
[CopyObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CopyObject.html)、
[Working with archived objects](https://docs.aws.amazon.com/AmazonS3/latest/userguide/archived-objects.html)、
[Archive retrieval options](https://docs.aws.amazon.com/AmazonS3/latest/userguide/restoring-objects-retrieval-options.html)、
[Lifecycle transitions](https://docs.aws.amazon.com/AmazonS3/latest/userguide/lifecycle-transition-general-considerations.html)、
[RestoreStatus](https://docs.aws.amazon.com/AmazonS3/latest/API/API_RestoreStatus.html)。
已知偏差：AWS 的 CopyObject 文档把来源归档的错误写成 `ObjectNotInActiveTierError`，但实际返回的是 `InvalidObjectState`（SDK 里常见的
“Operation is not valid for the source object's storage class”），这里按后者实现。
