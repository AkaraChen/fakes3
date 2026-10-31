using System.Net;
using System.Text;
using Minio;
using Minio.DataModel;
using Minio.DataModel.Args;
using Minio.DataModel.Tags;
using Minio.Exceptions;
using static Matrix.Check;

namespace Matrix;

public sealed class MinioMatrix
{
    readonly IMinioClient mc = MakeClient(Env.SecretKey);
    readonly string bucket = "m-minio-dotnet-" + RandHex(4);
    readonly List<string> extraBuckets = [];
    static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(60) };

    static IMinioClient MakeClient(string secret)
    {
        var u = new Uri(Env.Endpoint);
        return new MinioClient()
            .WithEndpoint(u.Host, u.Port)
            .WithSSL(u.Scheme == "https")
            .WithCredentials(Env.AccessKey, secret)
            .WithRegion(Env.Region)
            .WithTimeout(60_000)
            .Build();
    }

    static string? ErrCode(Exception e) => (e as MinioException)?.Response?.Code;

    static int? ErrStatus(Exception e) => e is MinioException { ServerResponse: not null } m ? (int)m.ServerResponse.StatusCode : null;

    /// Run fn, which must fail with one of the given S3 error codes, HTTP statuses or Minio exception types.
    static async Task ExpectError(Func<Task> fn, string[] codes, int[] statuses, params Type[] types)
    {
        try
        {
            await fn();
        }
        catch (Exception e) when (e is MinioException or AuthorizationException)
        {
            if (codes.Contains(ErrCode(e)) || (ErrStatus(e) is { } st && statuses.Contains(st)) || types.Any(t => t.IsInstanceOfType(e))) return;
            throw new AssertionFailed($"expected {string.Join("/", codes.Concat(statuses.Select(s => s.ToString())).Concat(types.Select(t => t.Name)))}, got {Describe(e)}");
        }
        throw new AssertionFailed($"expected {string.Join("/", codes.Concat(statuses.Select(s => s.ToString())))}, but the call succeeded");
    }

    Task<Minio.DataModel.Response.PutObjectResponse> Put(string key, byte[] data, string? contentType = null,
        Dictionary<string, string>? headers = null, string? b = null)
    {
        var args = new PutObjectArgs().WithBucket(b ?? bucket).WithObject(key)
            .WithStreamData(new MemoryStream(data)).WithObjectSize(data.Length);
        if (contentType != null) args = args.WithContentType(contentType);
        if (headers != null) args = args.WithHeaders(headers);
        return mc.PutObjectAsync(args);
    }

    Task<Minio.DataModel.Response.PutObjectResponse> Put(string key, string data, string? contentType = null,
        Dictionary<string, string>? headers = null, string? b = null)
        => Put(key, Encoding.UTF8.GetBytes(data), contentType, headers, b);

    async Task<(byte[] Data, ObjectStat Stat)> Get(string key, Func<GetObjectArgs, GetObjectArgs>? configure = null, string? b = null)
    {
        var ms = new MemoryStream();
        var args = new GetObjectArgs().WithBucket(b ?? bucket).WithObject(key)
            .WithCallbackStream(async (s, ct) => await s.CopyToAsync(ms, ct));
        if (configure != null) args = configure(args);
        var stat = await mc.GetObjectAsync(args);
        return (ms.ToArray(), stat);
    }

    async Task<string> GetString(string key, Func<GetObjectArgs, GetObjectArgs>? configure = null, string? b = null)
        => Encoding.UTF8.GetString((await Get(key, configure, b)).Data);

    Task<ObjectStat> Stat(string key) => mc.StatObjectAsync(new StatObjectArgs().WithBucket(bucket).WithObject(key));

    async Task<List<Item>> List(string b, string prefix, bool recursive = true, bool versions = false)
    {
        var items = new List<Item>();
        await foreach (var it in mc.ListObjectsEnumAsync(new ListObjectsArgs().WithBucket(b).WithPrefix(prefix).WithRecursive(recursive).WithVersions(versions)))
            items.Add(it);
        return items;
    }

    async Task<string> ExtraBucket(string suffix)
    {
        var name = $"{bucket}-{suffix}";
        await mc.MakeBucketAsync(new MakeBucketArgs().WithBucket(name));
        extraBuckets.Add(name);
        return name;
    }

    async Task EmptyBucket(string b)
    {
        try
        {
            foreach (var it in await List(b, "", true, true))
                await mc.RemoveObjectAsync(new RemoveObjectArgs().WithBucket(b).WithObject(it.Key).WithVersionId(it.VersionId));
        }
        catch (Exception)
        {
        }
        // Listed keys are what the SDK decoded; if the server's encoding-type=url output is wrong they
        // won't match the stored key, so also remove the special keys by name.
        var keys = (await List(b, "")).Select(i => i.Key).Concat(SpecialKeys).Distinct().ToList();
        if (keys.Count > 0) await mc.RemoveObjectsAsync(new RemoveObjectsArgs().WithBucket(b).WithObjects(keys));
        try
        {
            await foreach (var u in mc.ListIncompleteUploadsEnumAsync(new ListIncompleteUploadsArgs().WithBucket(b)))
                await mc.RemoveIncompleteUploadAsync(new RemoveIncompleteUploadArgs().WithBucket(b).WithObject(u.Key));
        }
        catch (Exception)
        {
        }
    }

    static string MetaOf(ObjectStat st, string name)
    {
        foreach (var (k, v) in st.MetaData)
            if (string.Equals(k, name, StringComparison.OrdinalIgnoreCase)) return v;
        foreach (var (k, v) in st.ExtraHeaders)
            if (string.Equals(k, name, StringComparison.OrdinalIgnoreCase)) return v;
        return "<missing>";
    }

    static readonly HashSet<string> StandardHeaders = new(StringComparer.OrdinalIgnoreCase)
    {
        "Content-Type", "Cache-Control", "Content-Disposition", "Content-Encoding", "Content-Language", "Expires",
    };

    /// User metadata as sorted "key=value". ObjectStat.MetaData holds user metadata with the
    /// x-amz-meta- prefix already stripped, mixed with a few standard headers.
    static IEnumerable<string> UserMeta(ObjectStat st) => st.MetaData
        .Where(kv => !StandardHeaders.Contains(kv.Key))
        .Select(kv => $"{kv.Key}={kv.Value}")
        .Order(StringComparer.Ordinal);

    static readonly string[] SpecialKeys = ["sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"];

    static SkipScenario Skip(string reason) => new(reason);

    IEnumerable<(string, ScenarioFn)> Scenarios()
    {
        yield return ("create_bucket", async ct => await mc.MakeBucketAsync(new MakeBucketArgs().WithBucket(bucket), ct));

        yield return ("head_bucket", async ct =>
            That(await mc.BucketExistsAsync(new BucketExistsArgs().WithBucket(bucket), ct), "BucketExists returned false"));

        // BucketExists maps a 404 HeadBucket to `false` and throws on anything else.
        yield return ("head_bucket_missing", async ct =>
            That(!await mc.BucketExistsAsync(new BucketExistsArgs().WithBucket("m-missing-" + RandHex(6)), ct), "BucketExists returned true"));

        yield return ("list_buckets", async ct =>
        {
            var names = ((await mc.ListBucketsAsync(ct)).Buckets ?? []).Select(b => b.Name).ToList();
            That(names.Contains(bucket), $"bucket {bucket} not in list ({names.Count} buckets)");
        });

        yield return ("put_get_small", async ct =>
        {
            var data = Encoding.UTF8.GetBytes("hello world");
            var r = await Put("small.txt", data);
            Eq(Unquote(r.Etag), Md5Hex(data), "PUT ETag");
            var (got, st) = await Get("small.txt");
            SeqEq(got, data, "body");
            Eq(Unquote(st.ETag), Md5Hex(data), "GET ETag");
        });

        yield return ("put_get_empty", async ct =>
        {
            await Put("empty.txt", Array.Empty<byte>());
            var (got, st) = await Get("empty.txt");
            Eq(st.Size, 0L, "Content-Length");
            Eq(got.Length, 0, "body length");
        });

        // minio-dotnet has no "single PUT" switch: PutObject switches to multipart at >= 5 MiB.
        yield return ("put_get_large", async ct =>
        {
            var data = Random(8 * MiB);
            await Put("large.bin", data);
            var (got, _) = await Get("large.bin");
            That(got.AsSpan().SequenceEqual(data), $"8 MiB body mismatch (got {got.Length} bytes)");
        });

        yield return ("special_keys", async ct =>
        {
            var keys = SpecialKeys;
            for (var i = 0; i < keys.Length; i++)
            {
                await Put(keys[i], $"special-{i}");
                Eq(await GetString(keys[i]), $"special-{i}", $"body of {keys[i]}");
            }
            var listed = (await List(bucket, "")).Select(i => i.Key).ToList();
            foreach (var k in keys) That(listed.Contains(k), $"\"{k}\" missing from listing");
        });

        yield return ("head_object_metadata", async ct =>
        {
            await Put("meta.txt", "meta", "text/plain; charset=utf-8", new()
            {
                ["Cache-Control"] = "max-age=3600",
                ["Content-Disposition"] = "attachment; filename=\"meta.txt\"",
                ["Content-Encoding"] = "gzip",
                ["x-amz-meta-color"] = "Blue",
                ["x-amz-meta-x-y"] = "z",
            });
            var h = await Stat("meta.txt");
            Eq(h.ContentType, "text/plain; charset=utf-8", "Content-Type");
            Eq(MetaOf(h, "Cache-Control"), "max-age=3600", "Cache-Control");
            Eq(MetaOf(h, "Content-Disposition"), "attachment; filename=\"meta.txt\"", "Content-Disposition");
            Eq(MetaOf(h, "Content-Encoding"), "gzip", "Content-Encoding");
            SeqEq(UserMeta(h), ["color=Blue", "x-y=z"], "user metadata");
        });

        // WithOffsetAndLength covers bytes=a-b and bytes=a-; suffix ranges need a raw Range header.
        // ObjectStat does not expose Content-Range, so only the body can be checked.
        // Note: minio-dotnet sends the Range header on its preliminary HEAD as well, and treats a 206 HEAD as an error.
        yield return ("get_range", async ct =>
        {
            await Put("range.bin", RangeData);
            var cases = new (string Hdr, int A, int B, Func<GetObjectArgs, GetObjectArgs> Cfg)[]
            {
                ("bytes=10-19", 10, 19, a => a.WithOffsetAndLength(10, 10)),
                ("bytes=-5", 95, 99, a => a.WithHeaders(new Dictionary<string, string> { ["Range"] = "bytes=-5" })),
                ("bytes=95-", 95, 99, a => a.WithOffsetAndLength(95, 0)),
            };
            foreach (var (hdr, a, b, cfg) in cases)
            {
                var (got, _) = await Get("range.bin", cfg);
                SeqEq(got, RangeData[a..(b + 1)], $"body for {hdr}");
            }
        });

        yield return ("get_range_invalid", async ct =>
        {
            await Put("range-invalid.bin", RangeData);
            await ExpectError(() => Get("range-invalid.bin", a => a.WithOffsetAndLength(200, 101)), ["InvalidRange"], [416]);
        });

        yield return ("get_missing_key", ct => ExpectError(() => Get("missing-" + RandHex(4)), ["NoSuchKey"], [404], typeof(ObjectNotFoundException)));

        // Note: minio-dotnet 7.0.0 silently drops WithMatchETag/WithNotMatchETag (no If-Match /
        // If-None-Match header is sent), so these two fail on every server.
        yield return ("get_if_none_match", async ct =>
        {
            var etag = (await Put("inm.txt", "if-none-match")).Etag;
            await ExpectError(() => Get("inm.txt", a => a.WithNotMatchETag(etag)), [], [304], typeof(Minio.Exceptions.RedirectionException));
        });

        yield return ("get_if_match_fail", async ct =>
        {
            await Put("im.txt", "if-match");
            await ExpectError(() => Get("im.txt", a => a.WithMatchETag("\"wrong\"")), ["PreconditionFailed"], [412], typeof(PreconditionFailedException));
        });

        // PutObjectArgs.WithHeaders turns every non-standard header into x-amz-meta-*.
        yield return ("put_if_none_match", _ => throw Skip("minio-dotnet PutObject cannot send If-None-Match (WithHeaders rewrites it to x-amz-meta-if-none-match)"));

        yield return ("delete_object", async ct =>
        {
            await Put("del.txt", "x");
            await mc.RemoveObjectAsync(new RemoveObjectArgs().WithBucket(bucket).WithObject("del.txt"), ct);
            await ExpectError(() => Stat("del.txt"), ["NoSuchKey", "NotFound"], [404], typeof(ObjectNotFoundException));
        });

        // RemoveObjectAsync does not expose the HTTP status; success == 2xx.
        yield return ("delete_missing_object", async ct =>
            await mc.RemoveObjectAsync(new RemoveObjectArgs().WithBucket(bucket).WithObject("never-" + RandHex(4)), ct));

        yield return ("delete_objects", async ct =>
        {
            string[] keys = ["bd/1", "bd/2", "bd/3"];
            foreach (var k in keys) await Put(k, k);
            var errs = (await mc.RemoveObjectsAsync(new RemoveObjectsArgs().WithBucket(bucket).WithObjects(keys.ToList()), ct)).ToList();
            That(errs.Count == 0, "errors: " + string.Join(", ", errs.Select(e => $"{e.Key}: {e.Code}")));
            foreach (var k in keys) await ExpectError(() => Stat(k), ["NoSuchKey", "NotFound"], [404], typeof(ObjectNotFoundException));
        });

        yield return ("list_v2_basic", async ct =>
        {
            var objs = new SortedDictionary<string, string>(StringComparer.Ordinal)
            {
                ["lb/a"] = "x", ["lb/b"] = string.Concat(Enumerable.Repeat("yy", 10)), ["lb/c"] = string.Concat(Enumerable.Repeat("zzz", 100)),
            };
            foreach (var (k, v) in objs.Reverse()) await Put(k, v);
            var items = await List(bucket, "lb/");
            SeqEq(items.Select(i => i.Key), objs.Keys, "keys");
            foreach (var o in items)
            {
                var d = Encoding.UTF8.GetBytes(objs[o.Key]);
                Eq(o.Size, (ulong)d.Length, $"Size of {o.Key}");
                Eq(Unquote(o.ETag), Md5Hex(d), $"ETag of {o.Key}");
            }
        });

        yield return ("list_v2_prefix_delimiter", async ct =>
        {
            var b = await ExtraBucket("ld");
            foreach (var k in new[] { "a/1", "a/2", "b/1", "c" }) await Put(k, k, null, null, b);
            var items = await List(b, "", recursive: false);
            SeqEq(items.Where(i => i.IsDir).Select(i => i.Key), ["a/", "b/"], "CommonPrefixes");
            SeqEq(items.Where(i => !i.IsDir).Select(i => i.Key), ["c"], "Contents");
        });

        yield return ("list_v2_pagination", _ => throw Skip("minio-dotnet ListObjects has no MaxKeys/ContinuationToken; pagination is internal"));

        yield return ("list_v2_start_after", _ => throw Skip("minio-dotnet ListObjectsArgs has no StartAfter"));

        yield return ("list_v1", _ => throw Skip("minio-dotnet ListObjectsArgs has no Marker (WithListObjectsV1 only switches the API version)"));

        yield return ("copy_object", async ct =>
        {
            await Put("copy-src.txt", "copy me", "text/x-copy", new() { ["x-amz-meta-color"] = "Blue" });
            await mc.CopyObjectAsync(new CopyObjectArgs().WithBucket(bucket).WithObject("copy-dst.txt")
                .WithCopyObjectSource(new CopySourceObjectArgs().WithBucket(bucket).WithObject("copy-src.txt")), ct);
            var (got, st) = await Get("copy-dst.txt");
            Eq(Encoding.UTF8.GetString(got), "copy me", "body");
            Eq(st.ContentType, "text/x-copy", "Content-Type");
            SeqEq(UserMeta(st), ["color=Blue"], "metadata");
        });

        yield return ("copy_object_replace_metadata", async ct =>
        {
            await Put("copyr-src.txt", "copy me", "text/x-old", new() { ["x-amz-meta-color"] = "Blue" });
            await mc.CopyObjectAsync(new CopyObjectArgs().WithBucket(bucket).WithObject("copyr-dst.txt")
                .WithCopyObjectSource(new CopySourceObjectArgs().WithBucket(bucket).WithObject("copyr-src.txt"))
                .WithReplaceMetadataDirective(true)
                .WithContentType("text/x-new")
                .WithHeaders(new Dictionary<string, string> { ["x-amz-meta-shape"] = "round" }), ct);
            var st = await Stat("copyr-dst.txt");
            SeqEq(UserMeta(st), ["shape=round"], "metadata");
            Eq(st.ContentType, "text/x-new", "Content-Type");
            Eq(await GetString("copyr-dst.txt"), "copy me", "body");
        });

        const string noMultipart = "minio-dotnet does not expose low-level multipart APIs (Create/UploadPart/ListParts/Complete/Abort)";
        yield return ("multipart_upload", _ => throw Skip(noMultipart));
        yield return ("multipart_abort", _ => throw Skip(noMultipart));
        yield return ("multipart_list_parts", _ => throw Skip(noMultipart));
        yield return ("multipart_too_small", _ => throw Skip(noMultipart));

        // PutObjectAsync is minio-dotnet's high-level API: it splits large streams into multipart uploads.
        yield return ("high_level_upload", async ct =>
        {
            var data = Random(20 * MiB);
            var src = Path.GetTempFileName();
            var dst = Path.GetTempFileName();
            try
            {
                await File.WriteAllBytesAsync(src, data, ct);
                await mc.PutObjectAsync(new PutObjectArgs().WithBucket(bucket).WithObject("hl.bin").WithFileName(src), ct);
                File.Delete(dst);
                await mc.GetObjectAsync(new GetObjectArgs().WithBucket(bucket).WithObject("hl.bin").WithFile(dst), ct);
                var got = await File.ReadAllBytesAsync(dst, ct);
                That(got.AsSpan().SequenceEqual(data), $"20 MiB round-trip mismatch (got {got.Length} bytes)");
                var st = await Stat("hl.bin");
                That(System.Text.RegularExpressions.Regex.IsMatch(Unquote(st.ETag), "^[0-9a-f]{32}-[0-9]+$"), $"ETag {st.ETag} is not a multipart ETag");
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
            var url = await mc.PresignedGetObjectAsync(new PresignedGetObjectArgs().WithBucket(bucket).WithObject("presigned-get.txt").WithExpiry(300));
            using var r = await Http.GetAsync(url, ct);
            var body = await r.Content.ReadAsStringAsync(ct);
            Eq((int)r.StatusCode, 200, $"HTTP status ({(body.Length > 120 ? body[..120] : body)})");
            Eq(body, "presigned get body", "body");
        });

        yield return ("presigned_put", async ct =>
        {
            var url = await mc.PresignedPutObjectAsync(new PresignedPutObjectArgs().WithBucket(bucket).WithObject("presigned-put.txt").WithExpiry(300));
            var content = new ByteArrayContent(Encoding.UTF8.GetBytes("presigned put body"));
            content.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue("application/octet-stream");
            using var r = await Http.PutAsync(url, content, ct);
            var body = await r.Content.ReadAsStringAsync(ct);
            Eq((int)r.StatusCode, 200, $"HTTP status ({(body.Length > 120 ? body[..120] : body)})");
            Eq(await GetString("presigned-put.txt"), "presigned put body", "body");
        });

        foreach (var algo in new[] { "crc32", "crc32c", "sha1", "sha256", "crc64nvme" })
            yield return ($"checksum_{algo}", _ => throw Skip("minio-dotnet has no ChecksumAlgorithm / ChecksumMode support"));

        yield return ("content_md5_bad", _ => throw Skip("minio-dotnet PutObject cannot send Content-MD5 (WithHeaders rewrites it to x-amz-meta-content-md5)"));

        yield return ("bad_credentials", ct => ExpectError(() => MakeClient("wrong-secret").ListBucketsAsync(ct),
            ["SignatureDoesNotMatch", "InvalidAccessKeyId"], [403], typeof(AuthorizationException)));

        yield return ("delete_bucket_not_empty", async ct =>
        {
            await Put("not-empty.txt", "x");
            await ExpectError(() => mc.RemoveBucketAsync(new RemoveBucketArgs().WithBucket(bucket), ct), ["BucketNotEmpty"], [409]);
        });

        // PutObjectResponse does not carry VersionId, so versions are discovered via ListObjects(WithVersions).
        yield return ("versioning", async ct =>
        {
            var b = await ExtraBucket("ver");
            await mc.SetVersioningAsync(new SetVersioningArgs().WithBucket(b).WithVersioningEnabled(), ct);
            var vc = await mc.GetVersioningAsync(new GetVersioningArgs().WithBucket(b), ct);
            Eq(vc?.Status, "Enabled", "versioning status");
            await Put("v.txt", "version one", null, null, b);
            await Put("v.txt", "version two", null, null, b);
            var vs = await List(b, "v.txt", true, true);
            Eq(vs.Count, 2, "listed version count");
            That(vs.All(v => !string.IsNullOrEmpty(v.VersionId) && v.VersionId != "null") && vs[0].VersionId != vs[1].VersionId,
                "bad version ids: " + string.Join(",", vs.Select(v => v.VersionId)));
            // Item.IsLatest is not reliably populated, so fetch every version by id.
            var bodies = new List<string>();
            foreach (var v in vs) bodies.Add(await GetString("v.txt", a => a.WithVersionId(v.VersionId), b));
            SeqEq(bodies.Order(StringComparer.Ordinal), ["version one", "version two"], "bodies by version id");
            Eq(await GetString("v.txt", null, b), "version two", "latest body");
        });

        yield return ("object_tagging", async ct =>
        {
            await Put("tagged.txt", "tags");
            await mc.SetObjectTagsAsync(new SetObjectTagsArgs().WithBucket(bucket).WithObject("tagged.txt")
                .WithTagging(Tagging.GetObjectTags(new Dictionary<string, string> { ["env"] = "test", ["team"] = "storage" })), ct);
            var got = await mc.GetObjectTagsAsync(new GetObjectTagsArgs().WithBucket(bucket).WithObject("tagged.txt"), ct);
            SeqEq(got.Tags.Select(kv => $"{kv.Key}={kv.Value}").Order(StringComparer.Ordinal), ["env=test", "team=storage"], "tag set");
        });

        yield return ("delete_bucket", async ct =>
        {
            await EmptyBucket(bucket);
            await mc.RemoveBucketAsync(new RemoveBucketArgs().WithBucket(bucket), ct);
            That(!await mc.BucketExistsAsync(new BucketExistsArgs().WithBucket(bucket), ct), "bucket still exists");
        });
    }

    static string Describe(Exception e)
    {
        var name = e.GetType().Name;
        var code = ErrCode(e);
        var st = ErrStatus(e);
        var msg = Runner.FirstLine((e as MinioException)?.Response?.Message ?? e.Message);
        return code != null || st != null ? $"{name}: {code} ({st}): {msg}" : $"{name}: {msg}";
    }

    public async Task RunAll()
    {
        var version = Runner.PackageVersion(typeof(MinioClient).Assembly);
        await Runner.Run("minio-dotnet", version, Scenarios(), Describe, async () =>
        {
            foreach (var b in extraBuckets.Prepend(bucket))
            {
                try
                {
                    await EmptyBucket(b);
                    await mc.RemoveBucketAsync(new RemoveBucketArgs().WithBucket(b));
                }
                catch (Exception)
                {
                }
            }
        });
    }
}
