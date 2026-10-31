# SDK 兼容矩阵规范

每个 SDK 对同一组场景跑一遍，分别打到 `bench/servers.sh list` 里的每个 S3 实现上，
结果汇总成 `matrix/MATRIX.md`。

## 目录与入口

```
matrix/
  SPEC.md               本文件
  run.sh                对所有（或指定的）server × 语言跑一遍
  report.py             汇总 results/ → MATRIX.md
  results/<server>/<sdk>.json
  python/ js/ go/ rust/ java/ dotnet/ ...   每种语言一个目录
    run.sh              入口：跑本语言下所有 SDK，每个 SDK 写一个结果文件
```

`<lang>/run.sh` 从环境变量读取配置，不接受其他参数：

| 变量 | 含义 |
|---|---|
| `S3_ENDPOINT` | 例如 `http://127.0.0.1:9100` |
| `S3_ACCESS_KEY` / `S3_SECRET_KEY` | 默认 `fakes3` / `fakes3secret` |
| `S3_REGION` | `us-east-1` |
| `MATRIX_SERVER` | server 名，例如 `fakes3`，用于决定结果路径 |
| `MATRIX_OUT` | 结果目录，即 `matrix/results/$MATRIX_SERVER` |

一律使用 path-style 寻址。依赖安装（`npm ci`、`uv sync`、`cargo build`、`mvn package`）要幂等、可缓存，
第二次运行不能重新下载依赖。

## 结果文件格式

`$MATRIX_OUT/<sdk>.json`：

```json
{
  "sdk": "boto3",
  "lang": "python",
  "version": "1.40.1",
  "server": "fakes3",
  "results": [
    {"scenario": "put_get_small", "status": "pass", "ms": 1.8},
    {"scenario": "presigned_put", "status": "fail", "ms": 3.1, "error": "SignatureDoesNotMatch: ..."},
    {"scenario": "object_tagging", "status": "skip", "error": "SDK has no tagging API"}
  ]
}
```

- `status`：`pass` | `fail` | `skip`。只有 **SDK 本身不提供该能力**时才能用 `skip`，并且要写明原因。
  server 返回错误一律算 `fail`。
- `error`：失败时取异常类型加消息的第一行，截断到 300 字符。
- 每个场景独立：一个场景失败（包括抛异常、超时）不能影响后续场景。每个场景超时 60 秒。
- 每个 SDK 使用自己独有的 bucket 名，例如 `m-<sdk>-<8 位随机>`，结束时尽力清理。

## 场景列表

场景 ID 是契约，所有语言必须使用完全相同的 ID。顺序不限，但每个场景要自己准备前置数据。
"断言"一栏写的是判定 pass 的条件，必须真的校验，不能只看有没有抛异常。

| ID | 操作 | 断言 |
|---|---|---|
| `create_bucket` | CreateBucket | 成功 |
| `head_bucket` | HeadBucket（存在） | 成功 |
| `head_bucket_missing` | HeadBucket（不存在） | 404 / NotFound / NoSuchBucket |
| `list_buckets` | ListBuckets | 结果包含刚创建的 bucket |
| `put_get_small` | PUT 11 字节 → GET | 内容一致；ETag 等于 MD5 hex |
| `put_get_empty` | PUT 0 字节 → GET | 内容为空，Content-Length 为 0 |
| `put_get_large` | 单次 PUT 8 MiB 随机数据 → GET | 内容一致 |
| `special_keys` | 键 `sp ace/üñí©ødé/a+b=c&d?e#f.txt`、`a//b`、`~tilde!*'()` 分别 PUT → GET → 出现在 List 中 | 全部一致 |
| `head_object_metadata` | PUT 时带 Content-Type、Cache-Control、Content-Disposition、Content-Encoding、用户元数据 `{"color":"Blue","x-y":"z"}` → HEAD | 全部原样返回（用户元数据的 key 为小写） |
| `get_range` | 对 100 字节对象请求 `bytes=10-19`、`bytes=-5`、`bytes=95-` | 内容和 Content-Range 正确 |
| `get_range_invalid` | 对 100 字节对象请求 `bytes=200-300` | 416 / InvalidRange |
| `get_missing_key` | GET 不存在的 key | NoSuchKey / 404 |
| `get_if_none_match` | GET 时带 If-None-Match: <当前 ETag> | 304 |
| `get_if_match_fail` | GET 时带 If-Match: "wrong" | 412 |
| `put_if_none_match` | 对已存在的 key 做 PUT，带 If-None-Match: * | 412；不存在的 key 则成功 |
| `delete_object` | DELETE → HEAD | HEAD 返回 404 |
| `delete_missing_object` | DELETE 不存在的 key | 成功（204） |
| `delete_objects` | 批量删除 3 个 key | 3 个都被删除 |
| `list_v2_basic` | PUT 3 个 key → ListObjectsV2 | key 有序，Size 和 ETag 正确 |
| `list_v2_prefix_delimiter` | key：`a/1 a/2 b/1 c` → prefix="" delimiter="/" | CommonPrefixes 为 [a/, b/]，Contents 为 [c] |
| `list_v2_pagination` | 25 个 key，MaxKeys=10 → 翻页 | 分 3 页，共 25 个，无重复，有序 |
| `list_v2_start_after` | StartAfter 设为中间的某个 key | 只返回它之后的 key |
| `list_v1` | ListObjects（v1）加 Marker | 正确 |
| `copy_object` | CopyObject → GET | 内容和元数据都被复制 |
| `copy_object_replace_metadata` | CopyObject，MetadataDirective=REPLACE，带新元数据 | 新元数据生效 |
| `multipart_upload` | Create → 3 个 part（5 MiB、5 MiB、1 KiB）→ Complete → GET | 内容一致；ETag 形如 `<hex>-3` |
| `multipart_abort` | Create → 上传 1 个 part → Abort → ListParts | ListParts 返回 NoSuchUpload |
| `multipart_list_parts` | 上传 2 个 part → ListParts | 编号、大小、ETag 正确 |
| `multipart_too_small` | 两个 1 KiB 的 part → Complete | EntityTooSmall |
| `high_level_upload` | 用该 SDK 的高层传输 API（TransferManager、upload_fileobj、lib-storage Upload、manager.Uploader……）上传 20 MiB → 下载 | 内容一致 |
| `presigned_get` | 生成 presigned GET URL → 用普通 HTTP 客户端获取 | 内容一致 |
| `presigned_put` | 生成 presigned PUT URL → 用普通 HTTP 客户端上传 → SDK GET | 内容一致 |
| `checksum_crc32` | PUT 时 ChecksumAlgorithm=CRC32 → GET 时开启 ChecksumMode | 成功，且返回的 CRC32 正确 |
| `checksum_crc32c` | 同上，使用 CRC32C | 同上 |
| `checksum_sha1` | 同上，使用 SHA1 | 同上 |
| `checksum_sha256` | 同上，使用 SHA256 | 同上 |
| `checksum_crc64nvme` | 同上，使用 CRC64NVME | 同上 |
| `content_md5_bad` | PUT 时故意带错误的 Content-MD5 | BadDigest / 400 |
| `bad_credentials` | 用错误的 secret key 发 ListBuckets | 403（SignatureDoesNotMatch / InvalidAccessKeyId） |
| `delete_bucket_not_empty` | DeleteBucket 一个非空 bucket | BucketNotEmpty / 409 |
| `versioning` | 开启版本控制 → 对同一 key PUT 两次 → ListObjectVersions → 按 VersionId GET 旧版本 | 2 个版本，内容正确 |
| `object_tagging` | PutObjectTagging → GetObjectTagging | 标签一致 |
| `delete_bucket` | 清空后 DeleteBucket → HeadBucket | 404 |

场景只能新增，不能改名。新增的场景需要同步更新本文件。
