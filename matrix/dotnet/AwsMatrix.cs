using System.Net;
using System.Reflection;
using System.Text;
using Amazon.Runtime;
using Amazon.S3;
using Amazon.S3.Model;
using Amazon.S3.Transfer;
using static Matrix.Check;

namespace Matrix;

public sealed class AwsMatrix
{
    readonly AmazonS3Client s3 = MakeClient(Env.SecretKey);
    readonly string bucket = "m-aws-dotnet-" + RandHex(4);
    readonly List<string> extraBuckets = [];
    static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(60) };

    static AmazonS3Client MakeClient(string secret)
    {
        var cfg = new AmazonS3Config
        {
            ServiceURL = Env.Endpoint,
            ForcePathStyle = true,
            AuthenticationRegion = Env.Region,
            Timeout = TimeSpan.FromSeconds(60),
            MaxErrorRetry = 2,
        };
        return new AmazonS3Client(new BasicAWSCredentials(Env.AccessKey, secret), cfg);
    }

    static async Task ExpectError(Func<Task> fn, string[] codes, params HttpStatusCode[] statuses)
    {
        try
        {
            await fn();
        }
        catch (AmazonS3Exception e)
        {
            if (codes.Contains(e.ErrorCode) || statuses.Contains(e.StatusCode)) return;
            throw new AssertionFailed($"expected {string.Join("/", codes.Concat(statuses.Select(s => ((int)s).ToString())))}, got {e.ErrorCode} ({(int)e.StatusCode}): {Runner.FirstLine(e.Message)}");
        }
        throw new AssertionFailed($"expected {string.Join("/", codes.Concat(statuses.Select(s => ((int)s).ToString())))}, but the call succeeded");
    }

    Task<PutObjectResponse> Put(string key, byte[] data, Action<PutObjectRequest>? configure = null, string? b = null)
    {
        var req = new PutObjectRequest { BucketName = b ?? bucket, Key = key, InputStream = new MemoryStream(data) };
        configure?.Invoke(req);
        return s3.PutObjectAsync(req);
    }

    Task<PutObjectResponse> Put(string key, string data, Action<PutObjectRequest>? configure = null, string? b = null)
        => Put(key, Encoding.UTF8.GetBytes(data), configure, b);

    static async Task<byte[]> ReadAll(GetObjectResponse r)
    {
        using (r)
        {
            var ms = new MemoryStream();
            await r.ResponseStream.CopyToAsync(ms);
            return ms.ToArray();
        }
    }

    async Task<byte[]> Get(string key, Action<GetObjectRequest>? configure = null, string? b = null)
    {
        var req = new GetObjectRequest { BucketName = b ?? bucket, Key = key };
        configure?.Invoke(req);
        return await ReadAll(await s3.GetObjectAsync(req));
    }

    Task<GetObjectMetadataResponse> Head(string key) => s3.GetObjectMetadataAsync(bucket, key);

    async Task<string> ExtraBucket(string suffix)
    {
        var name = $"{bucket}-{suffix}";
        await s3.PutBucketAsync(name);
        extraBuckets.Add(name);
        return name;
    }

    async Task EmptyBucket(string b)
    {
        try
        {
            string? keyMarker = null, versionMarker = null;
            while (true)
            {
                var r = await s3.ListVersionsAsync(new ListVersionsRequest { BucketName = b, KeyMarker = keyMarker, VersionIdMarker = versionMarker });
                var objs = (r.Versions ?? []).Select(v => new KeyVersion { Key = v.Key, VersionId = v.VersionId }).ToList();
                if (objs.Count > 0) await s3.DeleteObjectsAsync(new DeleteObjectsRequest { BucketName = b, Objects = objs, Quiet = true });
                if (r.IsTruncated != true) break;
                keyMarker = r.NextKeyMarker;
                versionMarker = r.NextVersionIdMarker;
            }
        }
        catch (AmazonS3Exception)
        {
        }
        string? token = null;
        while (true)
        {
            var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = b, ContinuationToken = token });
            var objs = (r.S3Objects ?? []).Select(o => new KeyVersion { Key = o.Key }).ToList();
            if (objs.Count > 0) await s3.DeleteObjectsAsync(new DeleteObjectsRequest { BucketName = b, Objects = objs, Quiet = true });
            if (r.IsTruncated != true) break;
            token = r.NextContinuationToken;
        }
        try
        {
            var ups = await s3.ListMultipartUploadsAsync(new ListMultipartUploadsRequest { BucketName = b });
            foreach (var u in ups.MultipartUploads ?? [])
                await s3.AbortMultipartUploadAsync(b, u.Key, u.UploadId);
        }
        catch (AmazonS3Exception)
        {
        }
    }

    async Task<(string Id, List<byte[]> Datas, List<PartETag> Parts)> UploadParts(string key, params int[] sizes)
    {
        var id = (await s3.InitiateMultipartUploadAsync(bucket, key)).UploadId;
        var datas = new List<byte[]>();
        var parts = new List<PartETag>();
        for (var i = 0; i < sizes.Length; i++)
        {
            var d = Random(sizes[i]);
            datas.Add(d);
            var r = await s3.UploadPartAsync(new UploadPartRequest
            {
                BucketName = bucket, Key = key, UploadId = id, PartNumber = i + 1, InputStream = new MemoryStream(d),
            });
            parts.Add(new PartETag(i + 1, r.ETag));
        }
        return (id, datas, parts);
    }

    async Task AbortQuietly(string key, string id)
    {
        try
        {
            await s3.AbortMultipartUploadAsync(bucket, key, id);
        }
        catch (Exception)
        {
        }
    }

    string Presign(string key, HttpVerb verb) => s3.GetPreSignedURL(new GetPreSignedUrlRequest
    {
        BucketName = bucket,
        Key = key,
        Verb = verb,
        Expires = DateTime.UtcNow.AddMinutes(5),
        Protocol = Env.Endpoint.StartsWith("https") ? Protocol.HTTPS : Protocol.HTTP,
    });

    IEnumerable<(string, ScenarioFn)> Scenarios()
    {
        yield return ("create_bucket", async ct => await s3.PutBucketAsync(new PutBucketRequest { BucketName = bucket }, ct));

        yield return ("head_bucket", async ct => await s3.HeadBucketAsync(new HeadBucketRequest { BucketName = bucket }, ct));

        yield return ("head_bucket_missing", ct => ExpectError(
            () => s3.HeadBucketAsync(new HeadBucketRequest { BucketName = "m-missing-" + RandHex(6) }, ct),
            ["NotFound", "NoSuchBucket"], HttpStatusCode.NotFound));

        yield return ("list_buckets", async ct =>
        {
            var names = ((await s3.ListBucketsAsync(new ListBucketsRequest(), ct)).Buckets ?? []).Select(b => b.BucketName).ToList();
            That(names.Contains(bucket), $"bucket {bucket} not in list ({names.Count} buckets)");
        });

        yield return ("put_get_small", async ct =>
        {
            var data = Encoding.UTF8.GetBytes("hello world");
            var r = await Put("small.txt", data);
            Eq(Unquote(r.ETag), Md5Hex(data), "PUT ETag");
            using var g = await s3.GetObjectAsync(bucket, "small.txt", ct);
            Eq(Unquote(g.ETag), Md5Hex(data), "GET ETag");
            SeqEq(await ReadAll(g), data, "body");
        });

        yield return ("put_get_empty", async ct =>
        {
            await Put("empty.txt", Array.Empty<byte>());
            using var g = await s3.GetObjectAsync(bucket, "empty.txt", ct);
            Eq(g.ContentLength, 0L, "Content-Length");
            Eq((await ReadAll(g)).Length, 0, "body length");
        });

        yield return ("put_get_large", async ct =>
        {
            var data = Random(8 * MiB);
            await Put("large.bin", data);
            var got = await Get("large.bin");
            That(got.AsSpan().SequenceEqual(data), $"8 MiB body mismatch (got {got.Length} bytes)");
        });

        yield return ("special_keys", async ct =>
        {
            string[] keys = ["sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"];
            for (var i = 0; i < keys.Length; i++)
            {
                await Put(keys[i], $"special-{i}");
                Eq(Encoding.UTF8.GetString(await Get(keys[i])), $"special-{i}", $"body of {keys[i]}");
            }
            var listed = new List<string>();
            string? token = null;
            do
            {
                var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = bucket, ContinuationToken = token }, ct);
                listed.AddRange((r.S3Objects ?? []).Select(o => o.Key));
                token = r.IsTruncated == true ? r.NextContinuationToken : null;
            } while (token != null);
            foreach (var k in keys) That(listed.Contains(k), $"\"{k}\" missing from listing");
        });

        yield return ("head_object_metadata", async ct =>
        {
            await Put("meta.txt", "meta", r =>
            {
                r.ContentType = "text/plain; charset=utf-8";
                r.Headers.CacheControl = "max-age=3600";
                r.Headers.ContentDisposition = "attachment; filename=\"meta.txt\"";
                r.Headers.ContentEncoding = "gzip";
                r.Metadata.Add("color", "Blue");
                r.Metadata.Add("x-y", "z");
            });
            var h = await Head("meta.txt");
            Eq(h.Headers.ContentType, "text/plain; charset=utf-8", "Content-Type");
            Eq(h.Headers.CacheControl, "max-age=3600", "Cache-Control");
            Eq(h.Headers.ContentDisposition, "attachment; filename=\"meta.txt\"", "Content-Disposition");
            Eq(h.Headers.ContentEncoding, "gzip", "Content-Encoding");
            var md = h.Metadata.Keys.OrderBy(k => k, StringComparer.Ordinal).Select(k => $"{k}={h.Metadata[k]}");
            SeqEq(md, ["x-amz-meta-color=Blue", "x-amz-meta-x-y=z"], "user metadata");
        });

        yield return ("get_range", async ct =>
        {
            await Put("range.bin", RangeData);
            foreach (var (hdr, a, b) in new[] { ("bytes=10-19", 10, 19), ("bytes=-5", 95, 99), ("bytes=95-", 95, 99) })
            {
                using var g = await s3.GetObjectAsync(new GetObjectRequest { BucketName = bucket, Key = "range.bin", ByteRange = new ByteRange(hdr) }, ct);
                Eq(g.ContentRange, $"bytes {a}-{b}/100", $"Content-Range for {hdr}");
                SeqEq(await ReadAll(g), RangeData[a..(b + 1)], $"body for {hdr}");
            }
        });

        yield return ("get_range_invalid", async ct =>
        {
            await Put("range-invalid.bin", RangeData);
            await ExpectError(() => Get("range-invalid.bin", r => r.ByteRange = new ByteRange("bytes=200-300")),
                ["InvalidRange"], HttpStatusCode.RequestedRangeNotSatisfiable);
        });

        yield return ("get_missing_key", ct => ExpectError(() => Get("missing-" + RandHex(4)), ["NoSuchKey"], HttpStatusCode.NotFound));

        yield return ("get_if_none_match", async ct =>
        {
            var etag = (await Put("inm.txt", "if-none-match")).ETag;
            await ExpectError(() => Get("inm.txt", r => r.EtagToNotMatch = etag), [], HttpStatusCode.NotModified);
        });

        yield return ("get_if_match_fail", async ct =>
        {
            await Put("im.txt", "if-match");
            await ExpectError(() => Get("im.txt", r => r.EtagToMatch = "\"wrong\""), ["PreconditionFailed"], HttpStatusCode.PreconditionFailed);
        });

        yield return ("put_if_none_match", async ct =>
        {
            var key = "pinm-" + RandHex(4);
            await Put(key, "first", r => r.IfNoneMatch = "*");
            await ExpectError(() => Put(key, "second", r => r.IfNoneMatch = "*"), ["PreconditionFailed"], HttpStatusCode.PreconditionFailed);
            Eq(Encoding.UTF8.GetString(await Get(key)), "first", "body after rejected overwrite");
        });

        yield return ("delete_object", async ct =>
        {
            await Put("del.txt", "x");
            await s3.DeleteObjectAsync(bucket, "del.txt", ct);
            await ExpectError(() => Head("del.txt"), ["NotFound", "NoSuchKey"], HttpStatusCode.NotFound);
        });

        yield return ("delete_missing_object", async ct =>
        {
            var r = await s3.DeleteObjectAsync(bucket, "never-" + RandHex(4), ct);
            Eq(r.HttpStatusCode, HttpStatusCode.NoContent, "status");
        });

        yield return ("delete_objects", async ct =>
        {
            string[] keys = ["bd/1", "bd/2", "bd/3"];
            foreach (var k in keys) await Put(k, k);
            var r = await s3.DeleteObjectsAsync(new DeleteObjectsRequest
            {
                BucketName = bucket, Objects = keys.Select(k => new KeyVersion { Key = k }).ToList(),
            }, ct);
            That((r.DeleteErrors ?? []).Count == 0, "errors: " + string.Join(", ", (r.DeleteErrors ?? []).Select(e => $"{e.Key}: {e.Code}")));
            SeqEq((r.DeletedObjects ?? []).Select(d => d.Key).Order(StringComparer.Ordinal), keys, "deleted keys");
            foreach (var k in keys) await ExpectError(() => Head(k), ["NotFound", "NoSuchKey"], HttpStatusCode.NotFound);
        });

        yield return ("list_v2_basic", async ct =>
        {
            var objs = new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["lb/a"] = "x", ["lb/b"] = string.Concat(Enumerable.Repeat("yy", 10)), ["lb/c"] = string.Concat(Enumerable.Repeat("zzz", 100)),
            };
            foreach (var (k, v) in objs.Reverse()) await Put(k, v);
            var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = bucket, Prefix = "lb/" }, ct);
            var contents = r.S3Objects ?? [];
            SeqEq(contents.Select(o => o.Key), objs.Keys, "keys");
            Eq(r.KeyCount, 3, "KeyCount");
            foreach (var o in contents)
            {
                var d = Encoding.UTF8.GetBytes(objs[o.Key]);
                Eq(o.Size, (long)d.Length, $"Size of {o.Key}");
                Eq(Unquote(o.ETag), Md5Hex(d), $"ETag of {o.Key}");
            }
        });

        yield return ("list_v2_prefix_delimiter", async ct =>
        {
            var b = await ExtraBucket("ld");
            foreach (var k in new[] { "a/1", "a/2", "b/1", "c" }) await Put(k, k, null, b);
            var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = b, Prefix = "", Delimiter = "/" }, ct);
            SeqEq(r.CommonPrefixes, ["a/", "b/"], "CommonPrefixes");
            SeqEq((r.S3Objects ?? []).Select(o => o.Key), ["c"], "Contents");
        });

        yield return ("list_v2_pagination", async ct =>
        {
            var keys = Enumerable.Range(0, 25).Select(i => $"pg/k{i:D2}").ToList();
            foreach (var k in keys) await Put(k, k);
            var got = new List<string>();
            var pages = 0;
            string? token = null;
            while (true)
            {
                var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = bucket, Prefix = "pg/", MaxKeys = 10, ContinuationToken = token }, ct);
                pages++;
                var c = r.S3Objects ?? [];
                That(c.Count <= 10, $"page {pages} has {c.Count} keys");
                got.AddRange(c.Select(o => o.Key));
                if (r.IsTruncated != true) break;
                token = r.NextContinuationToken;
                That(!string.IsNullOrEmpty(token), "truncated page without NextContinuationToken");
                That(pages < 10, "too many pages");
            }
            Eq(pages, 3, "page count");
            SeqEq(got, keys, "keys across pages");
        });

        yield return ("list_v2_start_after", async ct =>
        {
            for (var i = 0; i < 5; i++) await Put($"sa/k{i}", $"k{i}");
            var r = await s3.ListObjectsV2Async(new ListObjectsV2Request { BucketName = bucket, Prefix = "sa/", StartAfter = "sa/k2" }, ct);
            SeqEq((r.S3Objects ?? []).Select(o => o.Key), ["sa/k3", "sa/k4"], "keys");
        });

        yield return ("list_v1", async ct =>
        {
            for (var i = 0; i < 5; i++) await Put($"l1/k{i}", $"k{i}");
            var r = await s3.ListObjectsAsync(new ListObjectsRequest { BucketName = bucket, Prefix = "l1/", Marker = "l1/k1" }, ct);
            SeqEq((r.S3Objects ?? []).Select(o => o.Key), ["l1/k2", "l1/k3", "l1/k4"], "keys after marker");
            r = await s3.ListObjectsAsync(new ListObjectsRequest { BucketName = bucket, Prefix = "l1/", MaxKeys = 2 }, ct);
            SeqEq((r.S3Objects ?? []).Select(o => o.Key), ["l1/k0", "l1/k1"], "first page");
            That(r.IsTruncated == true, "IsTruncated should be true with MaxKeys=2");
        });

        yield return ("copy_object", async ct =>
        {
            await Put("copy-src.txt", "copy me", r =>
            {
                r.ContentType = "text/x-copy";
                r.Metadata.Add("color", "Blue");
            });
            await s3.CopyObjectAsync(new CopyObjectRequest
            {
                SourceBucket = bucket, SourceKey = "copy-src.txt", DestinationBucket = bucket, DestinationKey = "copy-dst.txt",
            }, ct);
            using var g = await s3.GetObjectAsync(bucket, "copy-dst.txt", ct);
            Eq(g.Headers.ContentType, "text/x-copy", "Content-Type");
            SeqEq(g.Metadata.Keys.Select(k => $"{k}={g.Metadata[k]}"), ["x-amz-meta-color=Blue"], "metadata");
            Eq(Encoding.UTF8.GetString(await ReadAll(g)), "copy me", "body");
        });

        yield return ("copy_object_replace_metadata", async ct =>
        {
            await Put("copyr-src.txt", "copy me", r =>
            {
                r.ContentType = "text/x-old";
                r.Metadata.Add("color", "Blue");
            });
            var req = new CopyObjectRequest
            {
                SourceBucket = bucket, SourceKey = "copyr-src.txt", DestinationBucket = bucket, DestinationKey = "copyr-dst.txt",
                MetadataDirective = S3MetadataDirective.REPLACE, ContentType = "text/x-new",
            };
            req.Metadata.Add("shape", "round");
            await s3.CopyObjectAsync(req, ct);
            var h = await Head("copyr-dst.txt");
            SeqEq(h.Metadata.Keys.Select(k => $"{k}={h.Metadata[k]}"), ["x-amz-meta-shape=round"], "metadata");
            Eq(h.Headers.ContentType, "text/x-new", "Content-Type");
            Eq(Encoding.UTF8.GetString(await Get("copyr-dst.txt")), "copy me", "body");
        });

        yield return ("multipart_upload", async ct =>
        {
            var (id, datas, parts) = await UploadParts("mpu.bin", 5 * MiB, 5 * MiB, 1024);
            var c = await s3.CompleteMultipartUploadAsync(new CompleteMultipartUploadRequest
            {
                BucketName = bucket, Key = "mpu.bin", UploadId = id, PartETags = parts,
            }, ct);
            That(System.Text.RegularExpressions.Regex.IsMatch(Unquote(c.ETag), "^[0-9a-f]{32}-3$"), $"Complete ETag {c.ETag} is not <md5>-3");
            using var g = await s3.GetObjectAsync(bucket, "mpu.bin", ct);
            That(System.Text.RegularExpressions.Regex.IsMatch(Unquote(g.ETag), "^[0-9a-f]{32}-3$"), $"GET ETag {g.ETag} is not <md5>-3");
            That((await ReadAll(g)).AsSpan().SequenceEqual(datas.SelectMany(d => d).ToArray()), "body mismatch");
        });

        yield return ("multipart_abort", async ct =>
        {
            var (id, _, _) = await UploadParts("mpa.bin", 1024);
            await s3.AbortMultipartUploadAsync(bucket, "mpa.bin", id, ct);
            await ExpectError(() => s3.ListPartsAsync(bucket, "mpa.bin", id, ct), ["NoSuchUpload"]);
        });

        yield return ("multipart_list_parts", async ct =>
        {
            var (id, datas, parts) = await UploadParts("mpl.bin", 5 * MiB, 1024);
            try
            {
                var r = await s3.ListPartsAsync(bucket, "mpl.bin", id, ct);
                var ps = r.Parts ?? [];
                SeqEq(ps.Select(p => p.PartNumber ?? -1), [1, 2], "part numbers");
                SeqEq(ps.Select(p => p.Size ?? -1), datas.Select(d => (long)d.Length), "part sizes");
                SeqEq(ps.Select(p => Unquote(p.ETag)), datas.Select(Md5Hex), "part ETags");
                SeqEq(ps.Select(p => Unquote(p.ETag)), parts.Select(p => Unquote(p.ETag)), "part ETags vs UploadPart");
            }
            finally
            {
                await AbortQuietly("mpl.bin", id);
            }
        });

        yield return ("multipart_too_small", async ct =>
        {
            var (id, _, parts) = await UploadParts("mps.bin", 1024, 1024);
            try
            {
                await ExpectError(() => s3.CompleteMultipartUploadAsync(new CompleteMultipartUploadRequest
                {
                    BucketName = bucket, Key = "mps.bin", UploadId = id, PartETags = parts,
                }, ct), ["EntityTooSmall"]);
            }
            finally
            {
                await AbortQuietly("mps.bin", id);
            }
        });

        yield return ("high_level_upload", async ct =>
        {
            var data = Random(20 * MiB);
            var src = Path.GetTempFileName();
            var dst = Path.GetTempFileName();
            try
            {
                await File.WriteAllBytesAsync(src, data, ct);
                var tu = new TransferUtility(s3);
                await tu.UploadAsync(new TransferUtilityUploadRequest
                {
                    FilePath = src, BucketName = bucket, Key = "hl.bin", PartSize = 8 * MiB,
                }, ct);
                File.Delete(dst);
                await tu.DownloadAsync(new TransferUtilityDownloadRequest { FilePath = dst, BucketName = bucket, Key = "hl.bin" }, ct);
                var got = await File.ReadAllBytesAsync(dst, ct);
                That(got.AsSpan().SequenceEqual(data), $"20 MiB round-trip mismatch (got {got.Length} bytes)");
            }
            finally
            {
                File.Delete(src);
                File.Delete(dst);
            }
        });

        yield return ("presigned_get", async ct =>
        {
            await Put("presigned-get.txt", "presigned get body");
            using var r = await Http.GetAsync(Presign("presigned-get.txt", HttpVerb.GET), ct);
            var body = await r.Content.ReadAsStringAsync(ct);
            Eq((int)r.StatusCode, 200, $"HTTP status ({(body.Length > 120 ? body[..120] : body)})");
            Eq(body, "presigned get body", "body");
        });

        yield return ("presigned_put", async ct =>
        {
            var content = new ByteArrayContent(Encoding.UTF8.GetBytes("presigned put body"));
            content.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue("application/octet-stream");
            using var r = await Http.PutAsync(Presign("presigned-put.txt", HttpVerb.PUT), content, ct);
            var body = await r.Content.ReadAsStringAsync(ct);
            Eq((int)r.StatusCode, 200, $"HTTP status ({(body.Length > 120 ? body[..120] : body)})");
            Eq(Encoding.UTF8.GetString(await Get("presigned-put.txt")), "presigned put body", "body");
        });

        foreach (var algo in new[] { "CRC32", "CRC32C", "SHA1", "SHA256", "CRC64NVME" })
        {
            yield return ($"checksum_{algo.ToLowerInvariant()}", async ct =>
            {
                var key = "cks-" + algo.ToLowerInvariant();
                var data = Encoding.UTF8.GetBytes(string.Concat(Enumerable.Repeat($"checksum payload for {algo} ", 100)));
                var want = Checksums.Expected(algo, data);
                var p = await Put(key, data, r => r.ChecksumAlgorithm = ChecksumAlgorithm.FindValue(algo));
                Eq(Member(p, algo), want, $"PUT response Checksum{algo}");
                using var g = await s3.GetObjectAsync(new GetObjectRequest { BucketName = bucket, Key = key, ChecksumMode = ChecksumMode.ENABLED }, ct);
                SeqEq(await ReadAll(g), data, "body");
                Eq(Member(g, algo), want, $"GET response Checksum{algo}");
            });
        }

        yield return ("content_md5_bad", async ct =>
        {
            var bad = Convert.ToBase64String(System.Security.Cryptography.MD5.HashData(Encoding.UTF8.GetBytes("something else")));
            await ExpectError(() => Put("bad-md5.txt", "payload", r => r.MD5Digest = bad), ["BadDigest", "InvalidDigest"], HttpStatusCode.BadRequest);
        });

        yield return ("bad_credentials", ct => ExpectError(
            () => MakeClient("wrong-secret").ListBucketsAsync(new ListBucketsRequest(), ct),
            ["SignatureDoesNotMatch", "InvalidAccessKeyId"], HttpStatusCode.Forbidden));

        yield return ("delete_bucket_not_empty", async ct =>
        {
            await Put("not-empty.txt", "x");
            await ExpectError(() => s3.DeleteBucketAsync(bucket, ct), ["BucketNotEmpty"], HttpStatusCode.Conflict);
        });

        yield return ("versioning", async ct =>
        {
            var b = await ExtraBucket("ver");
            await s3.PutBucketVersioningAsync(new PutBucketVersioningRequest
            {
                BucketName = b, VersioningConfig = new S3BucketVersioningConfig { Status = VersionStatus.Enabled },
            }, ct);
            Eq((await s3.GetBucketVersioningAsync(new GetBucketVersioningRequest { BucketName = b }, ct)).VersioningConfig?.Status?.Value, "Enabled", "versioning status");
            var v1 = (await Put("v.txt", "version one", null, b)).VersionId;
            var v2 = (await Put("v.txt", "version two", null, b)).VersionId;
            That(v1 != null && v2 != null && v1 != v2 && v1 != "null", $"PUT version ids: {v1}, {v2}");
            var vs = (await s3.ListVersionsAsync(new ListVersionsRequest { BucketName = b, Prefix = "v.txt" }, ct)).Versions ?? [];
            SeqEq(vs.Where(v => v.IsDeleteMarker != true).Select(v => v.VersionId).Order(StringComparer.Ordinal),
                new[] { v1!, v2! }.Order(StringComparer.Ordinal), "listed versions");
            Eq(Encoding.UTF8.GetString(await Get("v.txt", r => r.VersionId = v1, b)), "version one", "old version body");
            Eq(Encoding.UTF8.GetString(await Get("v.txt", null, b)), "version two", "latest body");
        });

        yield return ("object_tagging", async ct =>
        {
            await Put("tagged.txt", "tags");
            await s3.PutObjectTaggingAsync(new PutObjectTaggingRequest
            {
                BucketName = bucket, Key = "tagged.txt",
                Tagging = new Tagging { TagSet = [new Tag { Key = "env", Value = "test" }, new Tag { Key = "team", Value = "storage" }] },
            }, ct);
            var got = (await s3.GetObjectTaggingAsync(new GetObjectTaggingRequest { BucketName = bucket, Key = "tagged.txt" }, ct)).Tagging ?? [];
            SeqEq(got.Select(t => $"{t.Key}={t.Value}").Order(StringComparer.Ordinal), ["env=test", "team=storage"], "tag set");
        });

        yield return ("delete_bucket", async ct =>
        {
            await EmptyBucket(bucket);
            await s3.DeleteBucketAsync(bucket, ct);
            await ExpectError(() => s3.HeadBucketAsync(new HeadBucketRequest { BucketName = bucket }, ct), ["NotFound", "NoSuchBucket"], HttpStatusCode.NotFound);
        });
    }

    static string? Member(object resp, string algo) =>
        resp.GetType().GetProperty("Checksum" + algo)?.GetValue(resp) as string;

    static string Describe(Exception e) => e switch
    {
        AmazonS3Exception s => $"{s.GetType().Name}: {s.ErrorCode} ({(int)s.StatusCode}): {Runner.FirstLine(s.Message)}",
        _ => $"{e.GetType().Name}: {Runner.FirstLine(e.Message)}",
    };

    public async Task RunAll()
    {
        var asm = typeof(AmazonS3Client).Assembly;
        var version = asm.GetCustomAttribute<AssemblyFileVersionAttribute>()?.Version ?? Runner.PackageVersion(asm);
        await Runner.Run("aws-dotnet", version, Scenarios(), Describe, async () =>
        {
            foreach (var b in extraBuckets.Prepend(bucket))
            {
                try
                {
                    await EmptyBucket(b);
                    await s3.DeleteBucketAsync(b);
                }
                catch (Exception)
                {
                }
            }
        });
    }
}
