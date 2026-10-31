# SDK 兼容矩阵

由 `matrix/report.py` 根据 `matrix/results/` 生成，请勿手工编辑。场景定义见 [SPEC.md](SPEC.md)。

图例：✅ 通过　❌ 失败　➖ SDK 不支持该能力（skip）　· 未运行

## 汇总

通过率 = 通过 / (通过 + 失败)，不计 skip。

| Server | 通过 | 失败 | skip | 通过率 |
|---|---:|---:|---:|---:|
| fakes3 | 871 | 8 | 153 | 99.1% |
| minio | 847 | 32 | 153 | 96.4% |
| localstack | 803 | 76 | 153 | 91.4% |
| moto | 773 | 106 | 153 | 87.9% |
| s3mock | 751 | 128 | 153 | 85.4% |
| juicefs | 609 | 270 | 153 | 69.3% |

## 每个 SDK 的通过数

| 语言 | SDK | 版本 | fakes3 | minio | localstack | moto | s3mock | juicefs |
|---|---|---|---:|---:|---:|---:|---:|---:|
| dotnet | aws-dotnet | 4.0.104.8 | 43/43 | 42/43 | 41/43 | 41/43 | 40/43 | 10/43 |
| dotnet | minio-dotnet | 7.0.0 | 23/29 | 23/29 | 22/29 | 21/29 | 22/29 | 22/29 |
| go | aws-sdk-go | 1.55.8 | 42/42 | 41/42 | 41/42 | 40/42 | 35/42 | 33/42 |
| go | aws-sdk-go-v2 | 1.114.6 | 43/43 | 42/43 | 42/43 | 41/43 | 34/43 | 32/43 |
| go | gocloud-blob | 0.47.0 | 25/25 | 24/25 | 24/25 | 24/25 | 21/25 | 22/25 |
| go | minio-go | 7.4.1 | 43/43 | 42/43 | 41/43 | 36/43 | 35/43 | 33/43 |
| java | aws-java-v1 | 1.12.797 | 38/38 | 37/38 | 37/38 | 34/38 | 33/38 | 33/38 |
| java | aws-java-v2 | 2.57.8 | 43/43 | 42/43 | 41/43 | 40/43 | 39/43 | 9/43 |
| java | aws-java-v2-crt | 2.57.8 (aws-crt 0.49.0) | 43/43 | 37/43 | 41/43 | 36/43 | 39/43 | 9/43 |
| java | minio-java | 9.0.4 | 42/43 | 42/43 | 2/43 | 2/43 | 33/43 | 31/43 |
| js | aws-sdk-js-v2 | 2.1693.0 | 42/42 | 41/42 | 41/42 | 40/42 | 36/42 | 32/42 |
| js | aws-sdk-js-v3 | 3.1161.0 | 43/43 | 42/43 | 42/43 | 41/43 | 35/43 | 31/43 |
| js | bun-s3 | 1.4.4 | 18/18 | 17/18 | 17/18 | 17/18 | 17/18 | 17/18 |
| js | minio-js | 8.0.7 | 35/35 | 34/35 | 34/35 | 33/35 | 32/35 | 30/35 |
| php | aws-php | 3.402.5 | 43/43 | 42/43 | 42/43 | 41/43 | 35/43 | 31/43 |
| python | boto3 | 1.43.123 | 43/43 | 42/43 | 42/43 | 41/43 | 35/43 | 31/43 |
| python | minio-py | 7.2.20 | 36/36 | 35/36 | 35/36 | 35/36 | 34/36 | 32/36 |
| python | obstore | 0.12.1 | 24/25 | 24/25 | 23/25 | 21/25 | 23/25 | 17/25 |
| python | s3fs | 2026.9.0 | 36/36 | 35/36 | 35/36 | 34/36 | 29/36 | 25/36 |
| ruby | aws-ruby | 1.237.0 | 43/43 | 42/43 | 42/43 | 40/43 | 34/43 | 31/43 |
| rust | aws-sdk-s3 | 1.157.0 | 43/43 | 42/43 | 42/43 | 39/43 | 35/43 | 32/43 |
| rust | object_store | 0.14.2 | 24/24 | 24/24 | 23/24 | 23/24 | 23/24 | 18/24 |
| rust | opendal | 0.59.5 | 24/24 | 24/24 | 23/24 | 23/24 | 23/24 | 22/24 |
| rust | rust-s3 | 0.38.0 | 32/32 | 31/32 | 30/32 | 30/32 | 29/32 | 26/32 |

## fakes3

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_object_metadata` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_objects` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_basic` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `multipart_upload` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ |
| `high_level_upload` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_get` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ✅ | ➖ | ➖ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `bad_credentials` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_bucket_not_empty` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `object_tagging` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `delete_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | minio-java | AssertionError: [a//b] AssertionError: not in List: [] |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `get_range` | minio-dotnet | PartialContentException: Exception of type 'Minio.Exceptions.PartialContentException' was thrown. |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |

</details>

## minio

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ |
| `head_object_metadata` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_objects` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_basic` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `multipart_upload` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ |
| `high_level_upload` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_get` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ✅ | ➖ | ➖ | ✅ | ➖ | ✅ | ➖ | ✅ | ❌ | ✅ | ➖ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `bad_credentials` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_bucket_not_empty` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `object_tagging` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `delete_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `special_keys` | aws-dotnet | AmazonS3Exception: XMinioInvalidObjectName (400): Object name contains unsupported characters. |
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | aws-sdk-go | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `special_keys` | aws-sdk-go-v2 | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `special_keys` | gocloud-blob | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `special_keys` | minio-go | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `special_keys` | aws-java-v1 | AssertionError: [a//b] AmazonS3Exception: Object name contains unsupported characters. (Service: Amazon S3; Status Code: 400; Error Code: XMinioInvalidObjectName; Request ID: 18E3870DC35FD2DB; S3 Exte |
| `special_keys` | aws-java-v2 | AssertionError: [a//b] S3Exception: Object name contains unsupported characters. (Service: S3, Status Code: 400, Request ID: 18E3870BCB0C8543, Extended Request ID: 2007b11c225eb2520a138e595eb97e78e00e |
| `special_keys` | aws-java-v2-crt | AssertionError: [a//b] S3Exception: Object name contains unsupported characters. (Service: S3, Status Code: 400, Request ID: 18E3870CEE45E636, Extended Request ID: 2007b11c225eb2520a138e595eb97e78e00e |
| `special_keys` | minio-java | AssertionError: [a//b] ErrorResponseException: Object name contains unsupported characters. |
| `special_keys` | aws-sdk-js-v2 | XMinioInvalidObjectName: Object name contains unsupported characters. |
| `special_keys` | aws-sdk-js-v3 | XMinioInvalidObjectName: Object name contains unsupported characters. |
| `special_keys` | bun-s3 | S3Error(XMinioInvalidObjectName): Object name contains unsupported characters. |
| `special_keys` | minio-js | S3Error(XMinioInvalidObjectName): Object name contains unsupported characters. |
| `special_keys` | aws-php | S3Exception: XMinioInvalidObjectName (400): Object name contains unsupported characters. |
| `special_keys` | boto3 | ClientError: An error occurred (XMinioInvalidObjectName) when calling the PutObject operation: Object name contains unsupported characters. |
| `special_keys` | minio-py | S3Error: S3 operation failed; code: XMinioInvalidObjectName, message: Object name contains unsupported characters., resource: /m-miniopy-37b3c429/a//b, request_id: 18E3870A24846107, host_id: 2007b11c2 |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `special_keys` | s3fs | OSError: [Errno 5] An error occurred (XMinioInvalidObjectName) when calling the PutObject operation: Object name contains unsupported characters. |
| `special_keys` | aws-ruby | XMinioInvalidObjectName: (400) Object name contains unsupported characters. |
| `special_keys` | aws-sdk-s3 | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `special_keys` | rust-s3 | XMinioInvalidObjectName: Object name contains unsupported characters. (HTTP 400) |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `get_range` | minio-dotnet | PartialContentException: Exception of type 'Minio.Exceptions.PartialContentException' was thrown. |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `checksum_crc32` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc32: expected <ZQYiow==> got <null> |
| `checksum_crc32c` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc32c: expected <1knc8g==> got <null> |
| `checksum_sha1` | aws-java-v2-crt | AssertionError: x-amz-checksum-sha1: expected <Ia/V1H6ux31AuIoSdifDDVfRtMQ=> got <null> |
| `checksum_sha256` | aws-java-v2-crt | AssertionError: x-amz-checksum-sha256: expected <B7M7I0PSLyQ1jr6uV7VHuL7K6jrZ81qyZ4qshYu+jlA=> got <null> |
| `checksum_crc64nvme` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc64nvme: expected <HTT6eTgpLD0=> got <null> |

</details>

## localstack

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ❌ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_object_metadata` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_objects` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_basic` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `multipart_upload` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ |
| `high_level_upload` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_get` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ❌ | ➖ | ✅ | ✅ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ✅ | ➖ | ➖ | ✅ | ➖ | ✅ | ➖ | ✅ | ✅ | ❌ | ➖ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `bad_credentials` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `delete_bucket_not_empty` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `object_tagging` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `delete_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `create_bucket` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `create_bucket` | rust-s3 | InvalidLocationConstraint: The specified location-constraint is not valid (HTTP 400) |
| `head_bucket` | minio-java | AssertionError: bucketExists returned false |
| `list_buckets` | minio-java | AssertionError: bucket m-minio-java-7pyb4srv not in ListBuckets (0 buckets) |
| `put_get_small` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_get_empty` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_get_large` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | minio-java | AssertionError: [sp ace/üñí©ødé/a+b=c&d?e#f.txt] ErrorResponseException: The specified bucket does not exist; [a//b] ErrorResponseException: The specified bucket does not exist; [~tilde!*'()] ErrorRes |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `head_object_metadata` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_range` | minio-dotnet | PartialContentException: Exception of type 'Minio.Exceptions.PartialContentException' was thrown. |
| `get_range` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_range_invalid` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_none_match` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `get_if_match_fail` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_if_none_match` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_missing_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_objects` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_basic` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_prefix_delimiter` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `list_v2_pagination` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_start_after` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v1` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `copy_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `copy_object_replace_metadata` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_upload` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_abort` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_list_parts` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_too_small` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `high_level_upload` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `presigned_get` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `presigned_put` | minio-java | AssertionError: presigned PUT status 404: <?xml version='1.0' encoding='utf-8'?> |
| `checksum_crc32` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_crc32c` | aws-dotnet | InvalidRequestException: InvalidRequest (400): Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed. |
| `checksum_crc32c` | minio-go | InvalidRequest: Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed. (HTTP 400) |
| `checksum_crc32c` | aws-java-v2 | InvalidRequestException: Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed. (Service: S3, Status Code: 400, Request ID: 6d9044f9-d6df-4a9b-bbd2-05781c7ffec1, Extended  |
| `checksum_crc32c` | aws-java-v2-crt | InvalidRequestException: Expecting a single x-amz-checksum- header. Multiple checksum Types are not allowed. (Service: S3, Status Code: 400, Request ID: a624faae-ab72-402d-86fc-c4bc2c363152, Extended  |
| `checksum_crc32c` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_sha1` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_sha256` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_crc64nvme` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `content_md5_bad` | minio-java | AssertionError: PUT bad Content-MD5: expected [400]/[BadDigest, InvalidDigest] but got status=404 code=NoSuchBucket (ErrorResponseException: The specified bucket does not exist) |
| `bad_credentials` | aws-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | minio-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-sdk-go-v2 | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | gocloud-blob | AssertionError: List: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | minio-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-java-v1 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2-crt | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | minio-java | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-sdk-js-v2 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-sdk-js-v3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | bun-s3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-js | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-php | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | boto3 | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-py | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | obstore | AssertionFailed: operation: expected error PermissionDeniedError/UnauthenticatedError/BaseError, but it succeeded |
| `bad_credentials` | s3fs | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403/PermissionError, but it succeeded |
| `bad_credentials` | aws-ruby | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-s3 | AssertionError: expected error ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `bad_credentials` | object_store | AssertionError: expected 403, got success |
| `bad_credentials` | opendal | AssertionError: expected 403 PermissionDenied, got success |
| `bad_credentials` | rust-s3 | AssertionError: expected ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `delete_bucket_not_empty` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `versioning` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `object_tagging` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_bucket` | minio-java | ErrorResponseException: The specified bucket does not exist |

</details>

## moto

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ❌ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_object_metadata` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_objects` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_basic` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `multipart_upload` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ❌ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ |
| `high_level_upload` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ |
| `presigned_get` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ✅ | ➖ | ✅ | ✅ | ➖ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ✅ | ➖ | ✅ | ✅ | ➖ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ✅ | ➖ | ✅ | ✅ | ➖ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ✅ | ➖ | ✅ | ✅ | ➖ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ✅ | ➖ | ➖ | ✅ | ➖ | ❌ | ➖ | ✅ | ❌ | ❌ | ➖ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `bad_credentials` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `delete_bucket_not_empty` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `object_tagging` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `delete_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `create_bucket` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `create_bucket` | rust-s3 | InvalidLocationConstraint: The specified location-constraint is not valid (HTTP 400) |
| `head_bucket` | minio-java | AssertionError: bucketExists returned false |
| `list_buckets` | minio-java | AssertionError: bucket m-minio-java-ow2brr0d not in ListBuckets (0 buckets) |
| `put_get_small` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_get_empty` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_get_large` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | minio-java | AssertionError: [sp ace/üñí©ødé/a+b=c&d?e#f.txt] ErrorResponseException: The specified bucket does not exist; [a//b] ErrorResponseException: The specified bucket does not exist; [~tilde!*'()] ErrorRes |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `head_object_metadata` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_range` | minio-dotnet | PartialContentException: Exception of type 'Minio.Exceptions.PartialContentException' was thrown. |
| `get_range` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_range_invalid` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_none_match` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `get_if_match_fail` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `put_if_none_match` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_missing_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_objects` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_basic` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_prefix_delimiter` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `list_v2_pagination` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v2_start_after` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `list_v1` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `copy_object` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `copy_object_replace_metadata` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_upload` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_abort` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_list_parts` | aws-java-v1 | AssertionError: part count: expected <2> got <0> |
| `multipart_list_parts` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `multipart_list_parts` | minio-js | InvalidXMLError: Missing tag: "ListPartsResult" |
| `multipart_list_parts` | aws-sdk-s3 | HttpError: HTTP 200 service error: unhandled error: error parsing XML: encountered invalid XML root: expected ListPartsResult but got StartEl { name: Name { prefix: "", local: "ListPartsResponse" }, a |
| `multipart_too_small` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `high_level_upload` | aws-java-v1 | AssertionError: downloaded body: content mismatch (expected 20971520 bytes, got 36700160 bytes) |
| `high_level_upload` | aws-java-v2 | S3Exception: (Service: S3, Status Code: 100, Request ID: null) (SDK Attempt Count: 1) |
| `high_level_upload` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `high_level_upload` | aws-ruby | NoMethodError: TransferManager#download_file: undefined method 'match' for nil |
| `high_level_upload` | aws-sdk-s3 | streaming error: body checksum mismatch. expected body checksum to be e40185c4 but it was e5739decdc442539 |
| `presigned_get` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `presigned_put` | minio-java | AssertionError: presigned PUT status 404: <?xml version="1.0" encoding="utf-8"?> |
| `checksum_crc32` | minio-go | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc32: expected <TzxZEA==> got <null> |
| `checksum_crc32` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_crc32c` | minio-go | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc32c: expected <HwT3tg==> got <null> |
| `checksum_crc32c` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_sha1` | minio-go | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | aws-java-v2-crt | AssertionError: x-amz-checksum-sha1: expected <tHtRbkET4m8KB0ooBEp4SOewovM=> got <null> |
| `checksum_sha1` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_sha256` | minio-go | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | aws-java-v2-crt | AssertionError: x-amz-checksum-sha256: expected <BOHENM8lZAPnRKaKjQXPcsyykJENhciGf1mOJvTCTHU=> got <null> |
| `checksum_sha256` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_sha256` | obstore | AssertionFailed: ChecksumSHA256 (verified via HeadObject): expected 'Vo+1ur7Ndu4AnzjXkqENwjnEoSDWly3N+T94b4plP8E=', got None |
| `checksum_crc64nvme` | minio-go | AssertionError: GET checksum CRC64NVME: got "", want "JGIwLOqxo9M=" |
| `checksum_crc64nvme` | aws-java-v2-crt | AssertionError: x-amz-checksum-crc64nvme: expected <fX9eZqf1Eck=> got <null> |
| `checksum_crc64nvme` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `checksum_crc64nvme` | obstore | AssertionFailed: ChecksumCRC64NVME (verified via HeadObject): expected 'ENmEXMo5+F8=', got None |
| `content_md5_bad` | aws-dotnet | AssertionFailed: expected BadDigest/InvalidDigest/400, but the call succeeded |
| `content_md5_bad` | aws-sdk-go | AssertionError: PUT: expected error ([400] [BadDigest InvalidDigest]), got success |
| `content_md5_bad` | aws-sdk-go-v2 | AssertionError: PUT: expected error ([400] [BadDigest InvalidDigest]), got success |
| `content_md5_bad` | minio-go | AssertionError: PUT: expected error ([400] [BadDigest InvalidDigest]), got success |
| `content_md5_bad` | aws-java-v1 | AssertionError: PUT bad Content-MD5: expected [400]/[BadDigest, InvalidDigest] but got status=-1 code=null (SdkClientException: Unable to verify integrity of data upload. Client calculated content has |
| `content_md5_bad` | aws-java-v2 | AssertionError: PUT bad Content-MD5: expected error [400]/[BadDigest, InvalidDigest] but call succeeded |
| `content_md5_bad` | aws-java-v2-crt | AssertionError: PUT bad Content-MD5: expected error [400]/[BadDigest, InvalidDigest] but call succeeded |
| `content_md5_bad` | minio-java | AssertionError: PUT bad Content-MD5: expected [400]/[BadDigest, InvalidDigest] but got status=404 code=NoSuchBucket (ErrorResponseException: The specified bucket does not exist) |
| `content_md5_bad` | aws-sdk-js-v2 | AssertionError: operation: expected error BadDigest/400, but it succeeded |
| `content_md5_bad` | aws-sdk-js-v3 | AssertionError: operation: expected error BadDigest/400, but it succeeded |
| `content_md5_bad` | aws-php | AssertionFailed: expected BadDigest/InvalidDigest/400, but the call succeeded |
| `content_md5_bad` | boto3 | AssertionFailed: operation: expected error BadDigest/InvalidDigest/400, but it succeeded |
| `content_md5_bad` | s3fs | AssertionFailed: operation: expected error OSError, but it succeeded |
| `content_md5_bad` | aws-ruby | AssertionFailed: expected BadDigest/InvalidDigest/400, but the call succeeded |
| `content_md5_bad` | aws-sdk-s3 | AssertionError: expected error ["BadDigest", "InvalidDigest"]/[400], got success |
| `bad_credentials` | aws-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | minio-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-sdk-go-v2 | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | gocloud-blob | AssertionError: List: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | minio-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-java-v1 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2-crt | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | minio-java | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-sdk-js-v2 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-sdk-js-v3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | bun-s3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-js | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-php | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | boto3 | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-py | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | obstore | AssertionFailed: operation: expected error PermissionDeniedError/UnauthenticatedError/BaseError, but it succeeded |
| `bad_credentials` | s3fs | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403/PermissionError, but it succeeded |
| `bad_credentials` | aws-ruby | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-s3 | AssertionError: expected error ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `bad_credentials` | object_store | AssertionError: expected 403, got success |
| `bad_credentials` | opendal | AssertionError: expected 403 PermissionDenied, got success |
| `bad_credentials` | rust-s3 | AssertionError: expected ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `delete_bucket_not_empty` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `versioning` | minio-dotnet | InvalidOperationException: There is an error in XML document (2, 2). |
| `versioning` | minio-java | ErrorResponseException: The specified location-constraint is not valid |
| `object_tagging` | minio-java | ErrorResponseException: The specified bucket does not exist |
| `delete_bucket` | minio-java | ErrorResponseException: The specified bucket does not exist |

</details>

## s3mock

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ❌ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_object_metadata` | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ |
| `get_range` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_objects` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_basic` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ✅ |
| `multipart_upload` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ✅ |
| `high_level_upload` | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ |
| `presigned_get` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ✅ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ✅ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ✅ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ✅ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ✅ | ➖ | ➖ | ❌ | ➖ | ❌ | ➖ | ✅ | ✅ | ❌ | ➖ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ✅ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `bad_credentials` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `delete_bucket_not_empty` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ➖ | ➖ | ➖ |
| `object_tagging` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ❌ |
| `delete_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `create_bucket` | rust-s3 | HttpFailWithBody: {"detail":"Content-Type 'text/plain;charset=UTF-8' is not supported.","instance":"/m-rust-s3-c1cc8651","status":415,"title":"Unsupported Media Type"} (HTTP 415) |
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | aws-java-v1 | AssertionError: [a//b] AmazonS3Exception: (Service: Amazon S3; Status Code: 400; Error Code: 400 ; Request ID: null; S3 Extended Request ID: null; Proxy: null) |
| `special_keys` | aws-java-v2 | AssertionError: [a//b] S3Exception: (Service: S3, Status Code: 400, Request ID: null) (SDK Attempt Count: 1) |
| `special_keys` | minio-java | AssertionError: [a//b] AssertionError: not in List: [] |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `head_object_metadata` | aws-sdk-go | AssertionError: Content-Type: got "text/x-matrix;charset=utf-8", want "text/x-matrix; charset=utf-8" |
| `head_object_metadata` | aws-sdk-go-v2 | AssertionError: Content-Type: got "text/x-matrix;charset=utf-8", want "text/x-matrix; charset=utf-8" |
| `head_object_metadata` | gocloud-blob | AssertionError: Content-Type: got "text/x-matrix;charset=utf-8", want "text/x-matrix; charset=utf-8" |
| `head_object_metadata` | minio-go | AssertionError: Content-Type: got "text/x-matrix;charset=utf-8", want "text/x-matrix; charset=utf-8" |
| `head_object_metadata` | aws-java-v1 | AssertionError: Content-Type: expected <text/plain; charset=utf-8> got <text/plain;charset=utf-8> |
| `head_object_metadata` | aws-java-v2 | AssertionError: Content-Type: expected <text/plain; charset=utf-8> got <text/plain;charset=utf-8> |
| `head_object_metadata` | aws-java-v2-crt | AssertionError: Content-Type: expected <text/plain; charset=utf-8> got <text/plain;charset=utf-8> |
| `head_object_metadata` | minio-java | AssertionError: Content-Type: expected <text/plain; charset=utf-8> got <text/plain;charset=utf-8> |
| `head_object_metadata` | aws-php | AssertionFailed: Content-Type: expected 'text/plain; charset=utf-8', got 'text/plain;charset=utf-8' |
| `head_object_metadata` | aws-ruby | AssertionFailed: Content-Type: expected "text/plain; charset=utf-8", got "text/plain;charset=utf-8" |
| `get_range` | aws-dotnet | AmazonClientException: Expected hash not equal to calculated hash |
| `get_range` | aws-java-v2-crt | SdkClientException: Failed to send the request: Response missing required Content-Range header. (SDK Attempt Count: 1) |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `copy_object_replace_metadata` | aws-dotnet | AssertionFailed: Content-Type: expected "text/x-new", got "text/x-old" |
| `copy_object_replace_metadata` | minio-dotnet | AssertionFailed: Content-Type: expected "text/x-new", got "text/x-old" |
| `copy_object_replace_metadata` | aws-sdk-go | AssertionError: Content-Type: got "text/x-old", want "text/x-new" |
| `copy_object_replace_metadata` | aws-sdk-go-v2 | AssertionError: Content-Type: got "text/x-old", want "text/x-new" |
| `copy_object_replace_metadata` | gocloud-blob | AssertionError: Content-Type: got "text/x-old", want "text/x-new" |
| `copy_object_replace_metadata` | minio-go | AssertionError: Content-Type: got "text/x-old", want "text/x-new" |
| `copy_object_replace_metadata` | aws-java-v1 | AssertionError: replaced Content-Type: expected <text/x-replaced> got <application/octet-stream> |
| `copy_object_replace_metadata` | aws-java-v2 | AssertionError: replaced Content-Type: expected <text/x-replaced> got <application/octet-stream> |
| `copy_object_replace_metadata` | aws-java-v2-crt | AssertionError: replaced Content-Type: expected <text/x-replaced> got <application/octet-stream> |
| `copy_object_replace_metadata` | minio-java | AssertionError: replaced Content-Type: expected <text/x-replaced> got <application/octet-stream> |
| `copy_object_replace_metadata` | aws-sdk-js-v2 | AssertionError: ContentType: expected "application/x-new", got "application/x-old" |
| `copy_object_replace_metadata` | aws-sdk-js-v3 | AssertionError: ContentType: expected "application/x-new", got "application/x-old" |
| `copy_object_replace_metadata` | minio-js | AssertionError: content-type: expected "application/x-new", got "application/x-old" |
| `copy_object_replace_metadata` | aws-php | AssertionFailed: Content-Type: expected 'text/x-new', got 'text/x-old' |
| `copy_object_replace_metadata` | boto3 | AssertionFailed: ContentType: expected 'application/x-new', got 'application/x-old' |
| `copy_object_replace_metadata` | minio-py | AssertionFailed: Content-Type: expected 'application/x-new', got 'application/x-old' |
| `copy_object_replace_metadata` | s3fs | AssertionFailed: ContentType: expected 'application/x-new', got 'application/x-old' |
| `copy_object_replace_metadata` | aws-ruby | AssertionFailed: Content-Type: expected "text/x-new", got "text/x-old" |
| `copy_object_replace_metadata` | aws-sdk-s3 | AssertionError: Content-Type Some("text/x-old") |
| `high_level_upload` | aws-sdk-go-v2 | BadRequest: The upload was created using a crc32 checksum. The complete request must include the checksum for each part. It was missing for part 1 in the request. (HTTP 400) |
| `high_level_upload` | gocloud-blob | BadRequest: The upload was created using a crc32 checksum. The complete request must include the checksum for each part. It was missing for part 1 in the request. (HTTP 400) |
| `high_level_upload` | minio-java | ErrorResponseException: The upload was created using a crc32c checksum. The complete request must include the checksum for each part. It was missing for part 1 in the request. |
| `high_level_upload` | aws-sdk-js-v3 | BadRequest: The upload was created using a crc32 checksum. The complete request must include the checksum for each part. It was missing for part 1 in the request. |
| `high_level_upload` | boto3 | ClientError: An error occurred (BadRequest) when calling the CompleteMultipartUpload operation: The upload was created using a crc32 checksum. The complete request must include the checksum for each p |
| `high_level_upload` | aws-ruby | MultipartUploadError: multipart upload failed: The upload was created using a crc32 checksum. The complete request must include the checksum for each part. It was missing for part 1 in the request. |
| `checksum_crc32` | aws-sdk-go | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | aws-sdk-go-v2 | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | minio-go | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | minio-java | AssertionError: x-amz-checksum-crc32: expected <UBlyxg==> got <null> |
| `checksum_crc32` | aws-sdk-js-v2 | AssertionError: ChecksumCRC32: expected "ZSG1og==", got undefined |
| `checksum_crc32` | aws-sdk-js-v3 | AssertionError: ChecksumCRC32: expected "Hk68BQ==", got undefined |
| `checksum_crc32` | aws-php | AssertionFailed: PUT response ChecksumCRC32: expected '47rIBw==', got NULL |
| `checksum_crc32` | boto3 | AssertionFailed: ChecksumCRC32: expected 'eQpiqw==', got None |
| `checksum_crc32` | s3fs | AssertionFailed: ChecksumCRC32: expected 'x0RscA==', got None |
| `checksum_crc32` | aws-ruby | AssertionFailed: PUT response checksum_crc32: expected "47rIBw==", got nil |
| `checksum_crc32` | aws-sdk-s3 | AssertionError: checksum None != rI4zEQ== |
| `checksum_crc32c` | aws-sdk-go | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | aws-sdk-go-v2 | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | minio-go | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | minio-java | AssertionError: x-amz-checksum-crc32c: expected <0mFWcA==> got <null> |
| `checksum_crc32c` | aws-sdk-js-v2 | AssertionError: ChecksumCRC32C: expected "iMRNzA==", got undefined |
| `checksum_crc32c` | aws-sdk-js-v3 | AssertionError: ChecksumCRC32C: expected "YcXEiQ==", got undefined |
| `checksum_crc32c` | aws-php | AssertionFailed: PUT response ChecksumCRC32C: expected 'g/kujA==', got NULL |
| `checksum_crc32c` | boto3 | AssertionFailed: ChecksumCRC32C: expected 'cDZfdg==', got None |
| `checksum_crc32c` | s3fs | AssertionFailed: ChecksumCRC32C: expected 'kI3CYA==', got None |
| `checksum_crc32c` | aws-ruby | AssertionFailed: PUT response checksum_crc32c: expected "g/kujA==", got nil |
| `checksum_crc32c` | aws-sdk-s3 | AssertionError: checksum None != Q//BwA== |
| `checksum_sha1` | aws-sdk-go | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | aws-sdk-go-v2 | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | minio-go | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | minio-java | AssertionError: x-amz-checksum-sha1: expected <HONS8F2sExvfDBvOraBjhb7zl5g=> got <null> |
| `checksum_sha1` | aws-sdk-js-v2 | AssertionError: ChecksumSHA1: expected "OGXrBwU8h93/1cLc2jW2ebx4DAg=", got undefined |
| `checksum_sha1` | aws-sdk-js-v3 | AssertionError: ChecksumSHA1: expected "vPTqS8mkJlU3FRAIUy7Y3AAM3bE=", got undefined |
| `checksum_sha1` | aws-php | AssertionFailed: PUT response ChecksumSHA1: expected 'VEFvg6Az8vtSpXBzV2pxVmCqTSw=', got NULL |
| `checksum_sha1` | boto3 | AssertionFailed: ChecksumSHA1: expected 'RQf2mlwZosneeXo0YVQ5+mbdDKU=', got None |
| `checksum_sha1` | s3fs | AssertionFailed: ChecksumSHA1: expected 'QhAzXgK4f6dK+21r56s3YR3YGoQ=', got None |
| `checksum_sha1` | aws-ruby | AssertionFailed: PUT response checksum_sha1: expected "VEFvg6Az8vtSpXBzV2pxVmCqTSw=", got nil |
| `checksum_sha1` | aws-sdk-s3 | AssertionError: checksum None != XJW5Ce5B+m8cd1PZvPDhhgcwwcA= |
| `checksum_sha256` | aws-sdk-go | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | aws-sdk-go-v2 | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | minio-go | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | minio-java | AssertionError: x-amz-checksum-sha256: expected <Tt8Qq5AyyUbcqq3h+JQ/f/4PnbRERb5HfEpHV0/2Kg4=> got <null> |
| `checksum_sha256` | aws-sdk-js-v2 | AssertionError: ChecksumSHA256: expected "KvQBfrEkf0kUThAq+uyXPcwxRPsVGGk98sbt2jmwb38=", got undefined |
| `checksum_sha256` | aws-sdk-js-v3 | AssertionError: ChecksumSHA256: expected "Ry1OTe/VOscZeQWPXb5LljxMcVlema0j9SzfKT5vh5c=", got undefined |
| `checksum_sha256` | aws-php | AssertionFailed: PUT response ChecksumSHA256: expected 'QtqiG8v6SozQ6LK/LxVwvHshEr383u5UNqjFHrzlMvQ=', got NULL |
| `checksum_sha256` | boto3 | AssertionFailed: ChecksumSHA256: expected 'u1wxO+C//mhfAAyz90tq9to8qAbxc1sqCWcEMson18w=', got None |
| `checksum_sha256` | s3fs | AssertionFailed: ChecksumSHA256: expected 'VU2lLji0epSBtBMW2NJW4QzCYsChcT0W98a97uWBJ34=', got None |
| `checksum_sha256` | aws-ruby | AssertionFailed: PUT response checksum_sha256: expected "QtqiG8v6SozQ6LK/LxVwvHshEr383u5UNqjFHrzlMvQ=", got nil |
| `checksum_sha256` | aws-sdk-s3 | AssertionError: checksum None != mcdipJA1G2dK9fk1mfJD9z0O2JJF4HrXTRyTE4Rvvfs= |
| `checksum_crc64nvme` | aws-sdk-go-v2 | AssertionError: GET checksum CRC64NVME: got "", want "JGIwLOqxo9M=" |
| `checksum_crc64nvme` | minio-go | AssertionError: GET checksum CRC64NVME: got "", want "JGIwLOqxo9M=" |
| `checksum_crc64nvme` | minio-java | AssertionError: x-amz-checksum-crc64nvme: expected <YIE7FaMKKa0=> got <null> |
| `checksum_crc64nvme` | aws-sdk-js-v3 | AssertionError: ChecksumCRC64NVME: expected "q/rXqKJe9cI=", got undefined |
| `checksum_crc64nvme` | aws-php | AssertionFailed: PUT response ChecksumCRC64NVME: expected 'gQ8GHjFwvI8=', got NULL |
| `checksum_crc64nvme` | boto3 | AssertionFailed: ChecksumCRC64NVME: expected 'HsrRjsYUKgI=', got None |
| `checksum_crc64nvme` | s3fs | AssertionFailed: ChecksumCRC64NVME: expected 'ruhzh/L/qqo=', got None |
| `checksum_crc64nvme` | aws-ruby | AssertionFailed: PUT response checksum_crc64nvme: expected "gQ8GHjFwvI8=", got nil |
| `checksum_crc64nvme` | aws-sdk-s3 | AssertionError: checksum None != vLHk6kHwMwc= |
| `bad_credentials` | aws-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | minio-dotnet | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-sdk-go-v2 | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | gocloud-blob | AssertionError: List: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | minio-go | AssertionError: ListBuckets: expected error ([403] [SignatureDoesNotMatch InvalidAccessKeyId]), got success |
| `bad_credentials` | aws-java-v1 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2 | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-java-v2-crt | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | minio-java | AssertionError: ListBuckets with bad secret: expected error [403]/[SignatureDoesNotMatch, InvalidAccessKeyId] but call succeeded |
| `bad_credentials` | aws-sdk-js-v2 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-sdk-js-v3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | bun-s3 | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-js | AssertionError: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | aws-php | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | boto3 | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | minio-py | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403, but it succeeded |
| `bad_credentials` | obstore | AssertionFailed: operation: expected error PermissionDeniedError/UnauthenticatedError/BaseError, but it succeeded |
| `bad_credentials` | s3fs | AssertionFailed: operation: expected error SignatureDoesNotMatch/InvalidAccessKeyId/403/PermissionError, but it succeeded |
| `bad_credentials` | aws-ruby | AssertionFailed: expected SignatureDoesNotMatch/InvalidAccessKeyId/403, but the call succeeded |
| `bad_credentials` | aws-sdk-s3 | AssertionError: expected error ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `bad_credentials` | object_store | AssertionError: expected 403, got success |
| `bad_credentials` | opendal | AssertionError: expected 403 PermissionDenied, got success |
| `bad_credentials` | rust-s3 | AssertionError: expected ["SignatureDoesNotMatch", "InvalidAccessKeyId"]/[403], got success |
| `versioning` | aws-java-v1 | AssertionError: version count: expected <2> got <0> |
| `versioning` | minio-js | AssertionError: version count: expected 2, got 0 |
| `versioning` | aws-sdk-s3 | HttpError: HTTP 200 service error: unhandled error: error parsing XML: encountered invalid XML root: expected ListVersionsResult but got StartEl { name: Name { prefix: "", local: "ListBucketResult" }, |
| `object_tagging` | rust-s3 | HttpFailWithBody: {"detail":"Content-Type 'text/plain;charset=UTF-8' is not supported.","instance":"/m-rust-s3-c1cc8651/tagged","status":415,"title":"Unsupported Media Type"} (HTTP 415) |

</details>

## juicefs

| 场景 | aws-dotnet | minio-dotnet | aws-sdk-go | aws-sdk-go-v2 | gocloud-blob | minio-go | aws-java-v1 | aws-java-v2 | aws-java-v2-crt | minio-java | aws-sdk-js-v2 | aws-sdk-js-v3 | bun-s3 | minio-js | aws-php | boto3 | minio-py | obstore | s3fs | aws-ruby | aws-sdk-s3 | object_store | opendal | rust-s3 |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `create_bucket` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `head_bucket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `head_bucket_missing` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `list_buckets` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `put_get_small` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_empty` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_get_large` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `special_keys` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ |
| `head_object_metadata` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `get_range` | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_range_invalid` | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_missing_key` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_none_match` | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `get_if_match_fail` | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `put_if_none_match` | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `delete_object` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| `delete_missing_object` | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| `delete_objects` | ❌ | ✅ | ✅ | ❌ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `list_v2_basic` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_prefix_delimiter` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_pagination` | ❌ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v2_start_after` | ❌ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `list_v1` | ❌ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ |
| `copy_object` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `copy_object_replace_metadata` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `multipart_upload` | ❌ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `multipart_abort` | ❌ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_list_parts` | ❌ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `multipart_too_small` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ❌ |
| `high_level_upload` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_get` | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `presigned_put` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `checksum_crc32` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_crc32c` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_sha1` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_sha256` | ❌ | ➖ | ❌ | ❌ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `checksum_crc64nvme` | ❌ | ➖ | ➖ | ❌ | ➖ | ❌ | ➖ | ❌ | ❌ | ❌ | ➖ | ❌ | ➖ | ➖ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `content_md5_bad` | ✅ | ➖ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ➖ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ➖ |
| `bad_credentials` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `delete_bucket_not_empty` | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `versioning` | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ❌ | ➖ | ❌ | ❌ | ❌ | ➖ | ➖ | ➖ |
| `object_tagging` | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ✅ | ➖ | ➖ | ✅ |
| `delete_bucket` | ❌ | ✅ | ✅ | ✅ | ➖ | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ | ❌ | ➖ | ✅ | ❌ | ❌ | ✅ | ➖ | ❌ | ❌ | ✅ | ➖ | ➖ | ❌ |

<details><summary>失败详情</summary>

| 场景 | SDK | 错误 |
|---|---|---|
| `put_get_small` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `put_get_small` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB41CF572) (SDK Attempt Count: 1) |
| `put_get_small` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CA1BDB93F) (SDK Attempt Count: 1) |
| `put_get_empty` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB4BB49CD) (SDK Attempt Count: 1) |
| `put_get_empty` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CA2581F67) (SDK Attempt Count: 1) |
| `put_get_large` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `put_get_large` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB68F4472) (SDK Attempt Count: 1) |
| `put_get_large` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB6445CF6) (SDK Attempt Count: 1) |
| `special_keys` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `special_keys` | minio-dotnet | AssertionFailed: "a//b" missing from listing |
| `special_keys` | aws-sdk-go | AssertionError: key "a//b" missing from list |
| `special_keys` | aws-sdk-go-v2 | AssertionError: key "a//b" missing from list |
| `special_keys` | gocloud-blob | AssertionError: key "a//b" missing from list |
| `special_keys` | minio-go | AssertionError: key "a//b" missing from list |
| `special_keys` | aws-java-v1 | AssertionError: [a//b] AmazonS3Exception: Object name contains unsupported characters. (Service: Amazon S3; Status Code: 400; Error Code: XMinioInvalidObjectName; Request ID: 18E3873D42F73796; S3 Exte |
| `special_keys` | aws-java-v2 | AssertionError: [sp ace/üñí©ødé/a+b=c&d?e#f.txt] S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB708719E) |
| `special_keys` | aws-java-v2-crt | AssertionError: [sp ace/üñí©ødé/a+b=c&d?e#f.txt] S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB6B0A07F) |
| `special_keys` | minio-java | AssertionError: [a//b] ErrorResponseException: Object name contains unsupported characters. |
| `special_keys` | aws-sdk-js-v2 | AssertionError: key "a//b" missing from list |
| `special_keys` | aws-sdk-js-v3 | AssertionError: key "a//b" missing from list |
| `special_keys` | bun-s3 | AssertionError: key "a//b" missing from list |
| `special_keys` | minio-js | AssertionError: key "a//b" missing from list |
| `special_keys` | aws-php | AssertionFailed: 'a//b' missing from listing |
| `special_keys` | boto3 | AssertionFailed: key 'a//b' missing from list |
| `special_keys` | minio-py | AssertionFailed: key 'a//b' missing from list |
| `special_keys` | obstore | ValueError: Could not parse path: Path "a//b" contained empty path segment |
| `special_keys` | s3fs | AssertionFailed: key 'a//b' missing from list |
| `special_keys` | aws-ruby | AssertionFailed: "a//b" missing from listing |
| `special_keys` | aws-sdk-s3 | AssertionError: key "a//b" not in listing |
| `special_keys` | rust-s3 | AssertionError: key "a//b" not listed |
| `head_object_metadata` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `head_object_metadata` | minio-dotnet | UnexpectedMinioException: MinIO API responded with message=Unsupported signed header: (content-disposition: attachment; filename="meta.txt" |
| `head_object_metadata` | aws-sdk-go | AssertionError: Content-Encoding: got "", want "gzip" |
| `head_object_metadata` | aws-sdk-go-v2 | AssertionError: Content-Encoding: got "", want "gzip" |
| `head_object_metadata` | gocloud-blob | AssertionError: Content-Encoding: got "", want "gzip" |
| `head_object_metadata` | minio-go | AssertionError: Content-Encoding: got "", want "gzip" |
| `head_object_metadata` | aws-java-v1 | AssertionError: Content-Encoding: expected <gzip> got <null> |
| `head_object_metadata` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB7E7B11F) (SDK Attempt Count: 1) |
| `head_object_metadata` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB7A722BD) (SDK Attempt Count: 1) |
| `head_object_metadata` | minio-java | AssertionError: Content-Encoding: expected <gzip> got <null> |
| `head_object_metadata` | aws-sdk-js-v2 | AssertionError: ContentEncoding: expected "gzip", got undefined |
| `head_object_metadata` | aws-sdk-js-v3 | AssertionError: ContentEncoding: expected "gzip", got undefined |
| `head_object_metadata` | minio-js | AssertionError: content-encoding: expected "gzip", got undefined |
| `head_object_metadata` | aws-php | AssertionFailed: Content-Encoding: expected 'gzip', got NULL |
| `head_object_metadata` | boto3 | AssertionFailed: ContentEncoding: expected 'gzip', got None |
| `head_object_metadata` | minio-py | AssertionFailed: Content-Encoding: expected 'gzip', got None |
| `head_object_metadata` | obstore | AssertionFailed: Content-Encoding: expected 'gzip', got None |
| `head_object_metadata` | s3fs | AssertionFailed: ContentEncoding: expected 'gzip', got None |
| `head_object_metadata` | aws-ruby | AssertionFailed: Content-Encoding: expected "gzip", got nil |
| `head_object_metadata` | aws-sdk-s3 | AssertionError: Content-Encoding None |
| `head_object_metadata` | object_store | AssertionError: ContentEncoding: got None, want AttributeValue("gzip") |
| `head_object_metadata` | opendal | AssertionError: Content-Encoding None |
| `head_object_metadata` | rust-s3 | AssertionError: Content-Encoding None |
| `get_range` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `get_range` | minio-dotnet | PartialContentException: Exception of type 'Minio.Exceptions.PartialContentException' was thrown. |
| `get_range` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB825E035) (SDK Attempt Count: 1) |
| `get_range` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB7FBDA1E) (SDK Attempt Count: 1) |
| `get_range_invalid` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `get_range_invalid` | minio-dotnet | NullReferenceException: Object reference not set to an instance of an object. |
| `get_range_invalid` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB8850B51) (SDK Attempt Count: 1) |
| `get_range_invalid` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB84C27F6) (SDK Attempt Count: 1) |
| `get_if_none_match` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `get_if_none_match` | minio-dotnet | AssertionFailed: expected 304, but the call succeeded |
| `get_if_none_match` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BB9D6D2A8) (SDK Attempt Count: 1) |
| `get_if_none_match` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CB9DAF8C0) (SDK Attempt Count: 1) |
| `get_if_match_fail` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `get_if_match_fail` | minio-dotnet | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `get_if_match_fail` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBA452939) (SDK Attempt Count: 1) |
| `get_if_match_fail` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBA29D1A8) (SDK Attempt Count: 1) |
| `put_if_none_match` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `put_if_none_match` | aws-sdk-go | AssertionError: second PUT: expected error ([412] [PreconditionFailed]), got success |
| `put_if_none_match` | aws-sdk-go-v2 | AssertionError: second PUT: expected error ([412] [PreconditionFailed]), got success |
| `put_if_none_match` | gocloud-blob | AssertionError: second PUT: expected error ([412] [PreconditionFailed]), got success |
| `put_if_none_match` | minio-go | AssertionError: second PUT: expected error ([412] [PreconditionFailed]), got success |
| `put_if_none_match` | aws-java-v1 | AssertionError: PUT If-None-Match:* existing: expected error [412]/[PreconditionFailed] but call succeeded |
| `put_if_none_match` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBA91B58B) (SDK Attempt Count: 1) |
| `put_if_none_match` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBA95CC9D) (SDK Attempt Count: 1) |
| `put_if_none_match` | minio-java | AssertionError: PUT If-None-Match:* existing: expected error [412]/[PreconditionFailed] but call succeeded |
| `put_if_none_match` | aws-sdk-js-v2 | AssertionError: operation: expected error PreconditionFailed/412, but it succeeded |
| `put_if_none_match` | aws-sdk-js-v3 | AssertionError: operation: expected error PreconditionFailed/412, but it succeeded |
| `put_if_none_match` | minio-js | AssertionError: operation: expected error PreconditionFailed/412, but it succeeded |
| `put_if_none_match` | aws-php | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `put_if_none_match` | boto3 | AssertionFailed: operation: expected error PreconditionFailed/412, but it succeeded |
| `put_if_none_match` | obstore | AssertionFailed: operation: expected error AlreadyExistsError/PreconditionError, but it succeeded |
| `put_if_none_match` | s3fs | AssertionFailed: operation: expected error FileExistsError/OSError, but it succeeded |
| `put_if_none_match` | aws-ruby | AssertionFailed: expected PreconditionFailed/412, but the call succeeded |
| `put_if_none_match` | aws-sdk-s3 | AssertionError: expected error []/[412], got success |
| `put_if_none_match` | object_store | AssertionError: expected 412, got success |
| `put_if_none_match` | opendal | AssertionError: expected 412, got success |
| `put_if_none_match` | rust-s3 | AssertionError: expected []/[412], got success |
| `delete_object` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `delete_object` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBADCDFF9) (SDK Attempt Count: 1) |
| `delete_object` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBAE5D072) (SDK Attempt Count: 1) |
| `delete_object` | obstore | GenericError: Generic S3 error: Got invalid DeleteObjects response: missing field `Key` |
| `delete_object` | object_store | object_store::Error::Generic: Generic S3 error: Got invalid DeleteObjects response: missing field `Key` |
| `delete_missing_object` | obstore | GenericError: Generic S3 error: Got invalid DeleteObjects response: missing field `Key` |
| `delete_missing_object` | object_store | object_store::Error::Generic: Generic S3 error: Got invalid DeleteObjects response: missing field `Key` |
| `delete_objects` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `delete_objects` | aws-sdk-go-v2 | MissingContentMD5: Missing required header for this request: Content-Md5. (HTTP 400) |
| `delete_objects` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBBBF9C80) (SDK Attempt Count: 1) |
| `delete_objects` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBBDE7199) (SDK Attempt Count: 1) |
| `delete_objects` | minio-java | XmlParserException: org.simpleframework.xml.core.ValueRequiredException: Unable to satisfy @org.simpleframework.xml.Element(name="Key", type=void.class, data=false, required=true) on field 'name' priv |
| `delete_objects` | aws-sdk-js-v2 | AssertionError: Deleted: expected ["dm/1","dm/2","dm/3"], got [null,null,null] |
| `delete_objects` | aws-sdk-js-v3 | MissingContentMD5: Missing required header for this request: Content-Md5. |
| `delete_objects` | aws-php | S3Exception: MissingContentMD5 (400): Missing required header for this request: Content-Md5. |
| `delete_objects` | boto3 | ClientError: An error occurred (MissingContentMD5) when calling the DeleteObjects operation: Missing required header for this request: Content-Md5. |
| `delete_objects` | obstore | GenericError: Generic S3 error: Got invalid DeleteObjects response: missing field `Key` |
| `delete_objects` | s3fs | OSError: [Errno 5] An error occurred (MissingContentMD5) when calling the DeleteObjects operation: Missing required header for this request: Content-Md5. |
| `delete_objects` | aws-ruby | MissingContentMD5: (400) Missing required header for this request: Content-Md5. |
| `delete_objects` | aws-sdk-s3 | MissingContentMD5: Missing required header for this request: Content-Md5. (HTTP 400) |
| `delete_objects` | object_store | AssertionError: 1 results |
| `delete_objects` | rust-s3 | S3Error::SerdeXml: serde xml: missing field `Key` |
| `list_v2_basic` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `list_v2_basic` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBC2F7943) (SDK Attempt Count: 1) |
| `list_v2_basic` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBC2FFBBD) (SDK Attempt Count: 1) |
| `list_v2_prefix_delimiter` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `list_v2_prefix_delimiter` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBCF68441) (SDK Attempt Count: 1) |
| `list_v2_prefix_delimiter` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBCEBA525) (SDK Attempt Count: 1) |
| `list_v2_pagination` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `list_v2_pagination` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBD497719) (SDK Attempt Count: 1) |
| `list_v2_pagination` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBD2EBEE4) (SDK Attempt Count: 1) |
| `list_v2_start_after` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `list_v2_start_after` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBDBE1009) (SDK Attempt Count: 1) |
| `list_v2_start_after` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBD85ABAC) (SDK Attempt Count: 1) |
| `list_v1` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `list_v1` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBE23EEF6) (SDK Attempt Count: 1) |
| `list_v1` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBDB2FAE1) (SDK Attempt Count: 1) |
| `copy_object` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `copy_object` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBE86619C) (SDK Attempt Count: 1) |
| `copy_object` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBDDFF059) (SDK Attempt Count: 1) |
| `copy_object_replace_metadata` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `copy_object_replace_metadata` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BBEE28DC7) (SDK Attempt Count: 1) |
| `copy_object_replace_metadata` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CBE032CF4) (SDK Attempt Count: 1) |
| `multipart_upload` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `multipart_upload` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BC2350A38) (SDK Attempt Count: 1) |
| `multipart_upload` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CC202F75A) (SDK Attempt Count: 1) |
| `multipart_abort` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `multipart_abort` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BC35CF738) (SDK Attempt Count: 1) |
| `multipart_abort` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CC2C65425) (SDK Attempt Count: 1) |
| `multipart_list_parts` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `multipart_list_parts` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BC4726584) (SDK Attempt Count: 1) |
| `multipart_list_parts` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CC42D05B7) (SDK Attempt Count: 1) |
| `multipart_too_small` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `multipart_too_small` | aws-sdk-go | AssertionError: Complete: expected error ([] [EntityTooSmall]), got success |
| `multipart_too_small` | aws-sdk-go-v2 | AssertionError: Complete: expected error ([] [EntityTooSmall]), got success |
| `multipart_too_small` | minio-go | AssertionError: Complete: expected error ([] [EntityTooSmall]), got success |
| `multipart_too_small` | aws-java-v1 | AssertionError: Complete with 1 KiB parts: expected error []/[EntityTooSmall] but call succeeded |
| `multipart_too_small` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BC54FCF03) (SDK Attempt Count: 1) |
| `multipart_too_small` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CC5399CA4) (SDK Attempt Count: 1) |
| `multipart_too_small` | minio-java | AssertionError: Complete with 1 KiB parts: expected error []/[EntityTooSmall] but call succeeded |
| `multipart_too_small` | aws-sdk-js-v2 | AssertionError: operation: expected error EntityTooSmall, but it succeeded |
| `multipart_too_small` | aws-sdk-js-v3 | AssertionError: operation: expected error EntityTooSmall, but it succeeded |
| `multipart_too_small` | minio-js | AssertionError: operation: expected error EntityTooSmall, but it succeeded |
| `multipart_too_small` | aws-php | AssertionFailed: expected EntityTooSmall, but the call succeeded |
| `multipart_too_small` | boto3 | AssertionFailed: operation: expected error EntityTooSmall, but it succeeded |
| `multipart_too_small` | minio-py | AssertionFailed: operation: expected error EntityTooSmall, but it succeeded |
| `multipart_too_small` | aws-ruby | AssertionFailed: expected EntityTooSmall, but the call succeeded |
| `multipart_too_small` | aws-sdk-s3 | AssertionError: expected error ["EntityTooSmall"]/[], got success |
| `multipart_too_small` | object_store | AssertionError: expected EntityTooSmall, got success |
| `multipart_too_small` | rust-s3 | AssertionError: expected EntityTooSmall, got success: <?xml version="1.0" encoding="UTF-8"?> |
| `high_level_upload` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `high_level_upload` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD1EDB7E2) (SDK Attempt Count: 1) |
| `high_level_upload` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CC9EAE451) (SDK Attempt Count: 1) |
| `presigned_get` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `presigned_get` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD2D6B8A0) (SDK Attempt Count: 1) |
| `presigned_get` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CD99A3952) (SDK Attempt Count: 1) |
| `checksum_crc32` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `checksum_crc32` | aws-sdk-go | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | aws-sdk-go-v2 | AssertionError: GET checksum CRC32: got "", want "fq5Ozg==" |
| `checksum_crc32` | minio-go | XAmzContentSHA256Mismatch: The provided 'x-amz-content-sha256' header does not match what was computed. (HTTP 400) |
| `checksum_crc32` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD6BBACD7) (SDK Attempt Count: 1) |
| `checksum_crc32` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE0568CB1) (SDK Attempt Count: 1) |
| `checksum_crc32` | minio-java | AssertionError: x-amz-checksum-crc32: expected <d5khNw==> got <null> |
| `checksum_crc32` | aws-sdk-js-v2 | AssertionError: ChecksumCRC32: expected "L8C54w==", got undefined |
| `checksum_crc32` | aws-sdk-js-v3 | AssertionError: ChecksumCRC32: expected "L0xJfw==", got undefined |
| `checksum_crc32` | aws-php | AssertionFailed: PUT response ChecksumCRC32: expected '47rIBw==', got NULL |
| `checksum_crc32` | boto3 | AssertionFailed: ChecksumCRC32: expected 'nd3jWQ==', got None |
| `checksum_crc32` | s3fs | AssertionFailed: ChecksumCRC32: expected 'lAEKUw==', got None |
| `checksum_crc32` | aws-ruby | AssertionFailed: PUT response checksum_crc32: expected "47rIBw==", got nil |
| `checksum_crc32` | aws-sdk-s3 | AssertionError: checksum None != O6jgNw== |
| `checksum_crc32c` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `checksum_crc32c` | aws-sdk-go | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | aws-sdk-go-v2 | AssertionError: GET checksum CRC32C: got "", want "vVoBqw==" |
| `checksum_crc32c` | minio-go | XAmzContentSHA256Mismatch: The provided 'x-amz-content-sha256' header does not match what was computed. (HTTP 400) |
| `checksum_crc32c` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD73632F1) (SDK Attempt Count: 1) |
| `checksum_crc32c` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE0AFDD0A) (SDK Attempt Count: 1) |
| `checksum_crc32c` | minio-java | AssertionError: x-amz-checksum-crc32c: expected <3d4t0g==> got <null> |
| `checksum_crc32c` | aws-sdk-js-v2 | AssertionError: ChecksumCRC32C: expected "abKzjQ==", got undefined |
| `checksum_crc32c` | aws-sdk-js-v3 | AssertionError: ChecksumCRC32C: expected "6+w+hQ==", got undefined |
| `checksum_crc32c` | aws-php | AssertionFailed: PUT response ChecksumCRC32C: expected 'g/kujA==', got NULL |
| `checksum_crc32c` | boto3 | AssertionFailed: ChecksumCRC32C: expected 'zB97vg==', got None |
| `checksum_crc32c` | s3fs | AssertionFailed: ChecksumCRC32C: expected 'iYqeTQ==', got None |
| `checksum_crc32c` | aws-ruby | AssertionFailed: PUT response checksum_crc32c: expected "g/kujA==", got nil |
| `checksum_crc32c` | aws-sdk-s3 | AssertionError: checksum None != LDknQQ== |
| `checksum_sha1` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `checksum_sha1` | aws-sdk-go | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | aws-sdk-go-v2 | AssertionError: GET checksum SHA1: got "", want "vFswkxBv7OkdQQIM3fyP3KUBrXk=" |
| `checksum_sha1` | minio-go | XAmzContentSHA256Mismatch: The provided 'x-amz-content-sha256' header does not match what was computed. (HTTP 400) |
| `checksum_sha1` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD796F6D3) (SDK Attempt Count: 1) |
| `checksum_sha1` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE109549F) (SDK Attempt Count: 1) |
| `checksum_sha1` | minio-java | AssertionError: x-amz-checksum-sha1: expected <vGchiqr5R5HF9e7F3AoDuiFQAg4=> got <null> |
| `checksum_sha1` | aws-sdk-js-v2 | AssertionError: ChecksumSHA1: expected "6tZpORhLqG/5IZN3RFdLfvzSdwo=", got undefined |
| `checksum_sha1` | aws-sdk-js-v3 | AssertionError: ChecksumSHA1: expected "d3tpgppltot9LwqRk+NvvroHm14=", got undefined |
| `checksum_sha1` | aws-php | AssertionFailed: PUT response ChecksumSHA1: expected 'VEFvg6Az8vtSpXBzV2pxVmCqTSw=', got NULL |
| `checksum_sha1` | boto3 | AssertionFailed: ChecksumSHA1: expected 'HSZoi4SW2tiQBTzoJzRIftrPAaY=', got None |
| `checksum_sha1` | s3fs | AssertionFailed: ChecksumSHA1: expected '9LyhgP9VJ3kGZPhMoqgMjBrjcOE=', got None |
| `checksum_sha1` | aws-ruby | AssertionFailed: PUT response checksum_sha1: expected "VEFvg6Az8vtSpXBzV2pxVmCqTSw=", got nil |
| `checksum_sha1` | aws-sdk-s3 | AssertionError: checksum None != /+O0RaWg9vfKD7YD9XQdgvTWaHw= |
| `checksum_sha256` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `checksum_sha256` | aws-sdk-go | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | aws-sdk-go-v2 | AssertionError: GET checksum SHA256: got "", want "pEP6p9Ngvflw0+c9ajLGEGeW0lP34Yd56ZrWvtH5n2E=" |
| `checksum_sha256` | minio-go | XAmzContentSHA256Mismatch: The provided 'x-amz-content-sha256' header does not match what was computed. (HTTP 400) |
| `checksum_sha256` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BD7F18D80) (SDK Attempt Count: 1) |
| `checksum_sha256` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE17B9D75) (SDK Attempt Count: 1) |
| `checksum_sha256` | minio-java | AssertionError: x-amz-checksum-sha256: expected <oYBy5PWX9uwNSjazq95/KmVv7TqAxL6oVpHSUE/+AMQ=> got <null> |
| `checksum_sha256` | aws-sdk-js-v2 | AssertionError: ChecksumSHA256: expected "wvxA/9lDSUkOnl3Hjf99+WCTIyty3lu9lUEvVMRRiH4=", got undefined |
| `checksum_sha256` | aws-sdk-js-v3 | AssertionError: ChecksumSHA256: expected "Byh1lYIO03uz5XbJiSpWpTDRbnF4qLVCQBaJd369RW8=", got undefined |
| `checksum_sha256` | aws-php | AssertionFailed: PUT response ChecksumSHA256: expected 'QtqiG8v6SozQ6LK/LxVwvHshEr383u5UNqjFHrzlMvQ=', got NULL |
| `checksum_sha256` | boto3 | AssertionFailed: ChecksumSHA256: expected 'cqDnYFyik2Kvr6OcifRMKoVtjBOVMl+33OLADGGbQ5Y=', got None |
| `checksum_sha256` | obstore | AssertionFailed: ChecksumSHA256 (verified via HeadObject): expected 'SdVbZ1BMa9dxaKSKe+3rM71FGz9gcUhdvFoBhl6tMW8=', got None |
| `checksum_sha256` | s3fs | AssertionFailed: ChecksumSHA256: expected 'dh7hcYnIg6Ik21DfJ15YO00yFWfcl+2kfcn2OqtUt54=', got None |
| `checksum_sha256` | aws-ruby | AssertionFailed: PUT response checksum_sha256: expected "QtqiG8v6SozQ6LK/LxVwvHshEr383u5UNqjFHrzlMvQ=", got nil |
| `checksum_sha256` | aws-sdk-s3 | AssertionError: checksum None != EyCJVelTWazxgtXBzr0yNJpC2enCcs6ELICruhDuVzw= |
| `checksum_crc64nvme` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `checksum_crc64nvme` | aws-sdk-go-v2 | AssertionError: GET checksum CRC64NVME: got "", want "JGIwLOqxo9M=" |
| `checksum_crc64nvme` | minio-go | XAmzContentSHA256Mismatch: The provided 'x-amz-content-sha256' header does not match what was computed. (HTTP 400) |
| `checksum_crc64nvme` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BDE340B9F) (SDK Attempt Count: 1) |
| `checksum_crc64nvme` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE1B2803F) (SDK Attempt Count: 1) |
| `checksum_crc64nvme` | minio-java | AssertionError: x-amz-checksum-crc64nvme: expected <pkpTBQIwVQY=> got <null> |
| `checksum_crc64nvme` | aws-sdk-js-v3 | AssertionError: ChecksumCRC64NVME: expected "x4rmNKxVEOU=", got undefined |
| `checksum_crc64nvme` | aws-php | AssertionFailed: PUT response ChecksumCRC64NVME: expected 'gQ8GHjFwvI8=', got NULL |
| `checksum_crc64nvme` | boto3 | AssertionFailed: ChecksumCRC64NVME: expected 'MkvfuIYzuKk=', got None |
| `checksum_crc64nvme` | obstore | AssertionFailed: ChecksumCRC64NVME (verified via HeadObject): expected 'o/EelpYN23Y=', got None |
| `checksum_crc64nvme` | s3fs | AssertionFailed: ChecksumCRC64NVME: expected 'VObQFEEF1lk=', got None |
| `checksum_crc64nvme` | aws-ruby | AssertionFailed: PUT response checksum_crc64nvme: expected "gQ8GHjFwvI8=", got nil |
| `checksum_crc64nvme` | aws-sdk-s3 | AssertionError: checksum None != Wce7qsVLEIc= |
| `delete_bucket_not_empty` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `delete_bucket_not_empty` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BDF63F07B) (SDK Attempt Count: 1) |
| `delete_bucket_not_empty` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE2B26372) (SDK Attempt Count: 1) |
| `versioning` | aws-dotnet | AmazonS3Exception: NotImplemented (501): A header you provided implies functionality that is not implemented |
| `versioning` | minio-dotnet | NotImplementedException: A header you provided implies functionality that is not implemented |
| `versioning` | aws-sdk-go | NotImplemented: A header you provided implies functionality that is not implemented (HTTP 501) |
| `versioning` | aws-sdk-go-v2 | NotImplemented: A header you provided implies functionality that is not implemented (HTTP 501) |
| `versioning` | minio-go | NotImplemented: A header you provided implies functionality that is not implemented (HTTP 501) |
| `versioning` | aws-java-v1 | AmazonS3Exception: A header you provided implies functionality that is not implemented (Service: Amazon S3; Status Code: 501; Error Code: NotImplemented; Request ID: 18E3873DA0AC022E; S3 Extended Requ |
| `versioning` | aws-java-v2 | S3Exception: A header you provided implies functionality that is not implemented (Service: S3, Status Code: 501, Request ID: 18E3873BE0497BCA) (SDK Attempt Count: 1) |
| `versioning` | aws-java-v2-crt | S3Exception: A header you provided implies functionality that is not implemented (Service: S3, Status Code: 501, Request ID: 18E3873CE3B03D24) (SDK Attempt Count: 1) |
| `versioning` | minio-java | ErrorResponseException: A header you provided implies functionality that is not implemented |
| `versioning` | aws-sdk-js-v2 | NotImplemented: A header you provided implies functionality that is not implemented |
| `versioning` | aws-sdk-js-v3 | NotImplemented: A header you provided implies functionality that is not implemented |
| `versioning` | minio-js | S3Error(NotImplemented): A header you provided implies functionality that is not implemented |
| `versioning` | aws-php | S3Exception: NotImplemented (501): A header you provided implies functionality that is not implemented |
| `versioning` | boto3 | ClientError: An error occurred (NotImplemented) when calling the PutBucketVersioning operation: A header you provided implies functionality that is not implemented |
| `versioning` | minio-py | S3Error: S3 operation failed; code: NotImplemented, message: A header you provided implies functionality that is not implemented, resource: /m-miniopy-6198c06d-v, request_id: 18E38738E34C7A35, host_id |
| `versioning` | s3fs | OSError: [Errno 38] A header you provided implies functionality that is not implemented |
| `versioning` | aws-ruby | NotImplemented: (501) A header you provided implies functionality that is not implemented |
| `versioning` | aws-sdk-s3 | NotImplemented: A header you provided implies functionality that is not implemented (HTTP 501) |
| `object_tagging` | aws-dotnet | AmazonS3Exception: XAmzContentSHA256Mismatch (400): The provided 'x-amz-content-sha256' header does not match what was computed. |
| `object_tagging` | aws-java-v2 | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873BE09C7B8A) (SDK Attempt Count: 1) |
| `object_tagging` | aws-java-v2-crt | S3Exception: The provided 'x-amz-content-sha256' header does not match what was computed. (Service: S3, Status Code: 400, Request ID: 18E3873CE3F11DF5) (SDK Attempt Count: 1) |
| `delete_bucket` | aws-dotnet | AmazonS3Exception: MissingContentMD5 (400): Missing required header for this request: Content-Md5. |
| `delete_bucket` | aws-java-v2 | S3Exception: Missing required header for this request: Content-Md5. (Service: S3, Status Code: 400, Request ID: 18E3873BE20F98BD) (SDK Attempt Count: 1) |
| `delete_bucket` | aws-java-v2-crt | S3Exception: Missing required header for this request: Content-Md5. (Service: S3, Status Code: 400, Request ID: 18E3873CE5426A17) (SDK Attempt Count: 1) |
| `delete_bucket` | minio-java | XmlParserException: org.simpleframework.xml.core.ValueRequiredException: Unable to satisfy @org.simpleframework.xml.Element(name="Key", type=void.class, data=false, required=true) on field 'name' priv |
| `delete_bucket` | aws-sdk-js-v3 | MissingContentMD5: Missing required header for this request: Content-Md5. |
| `delete_bucket` | aws-php | S3Exception: MissingContentMD5 (400): Missing required header for this request: Content-Md5. |
| `delete_bucket` | boto3 | ClientError: An error occurred (MissingContentMD5) when calling the DeleteObjects operation: Missing required header for this request: Content-Md5. |
| `delete_bucket` | s3fs | OSError: [Errno 5] An error occurred (MissingContentMD5) when calling the DeleteObjects operation: Missing required header for this request: Content-Md5. |
| `delete_bucket` | aws-ruby | MissingContentMD5: (400) Missing required header for this request: Content-Md5. |
| `delete_bucket` | rust-s3 | S3Error::SerdeXml: serde xml: missing field `Key` |

</details>

