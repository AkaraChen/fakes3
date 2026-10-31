package matrix;

import com.amazonaws.AmazonServiceException;
import com.amazonaws.HttpMethod;
import com.amazonaws.auth.AWSStaticCredentialsProvider;
import com.amazonaws.auth.BasicAWSCredentials;
import com.amazonaws.client.builder.AwsClientBuilder.EndpointConfiguration;
import com.amazonaws.services.s3.AmazonS3;
import com.amazonaws.services.s3.AmazonS3ClientBuilder;
import com.amazonaws.services.s3.model.*;
import com.amazonaws.services.s3.transfer.TransferManager;
import com.amazonaws.services.s3.transfer.TransferManagerBuilder;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.nio.file.Files;
import java.util.*;

/** aws-sdk-java (v1): AmazonS3Client + TransferManager. */
final class V1Suite extends Suite {
    private final AmazonS3 s3;
    private final List<String> extraBuckets = Collections.synchronizedList(new ArrayList<>());

    V1Suite(Env env, String sdk) {
        super(env, sdk);
        // Range requests built via custom header (suffix ranges have no typed API) would otherwise trip
        // v1's whole-object MD5 validation of GET responses.
        System.setProperty("com.amazonaws.services.s3.disableGetObjectMD5Validation", "true");
        s3 = client(env.secretKey());
    }

    private AmazonS3 client(String secret) {
        return AmazonS3ClientBuilder.standard()
                .withEndpointConfiguration(new EndpointConfiguration(env.endpoint(), env.region()))
                .withPathStyleAccessEnabled(true)
                .withCredentials(new AWSStaticCredentialsProvider(new BasicAWSCredentials(env.accessKey(), secret)))
                .build();
    }

    @Override
    int status(Throwable t) { return t instanceof AmazonServiceException e ? e.getStatusCode() : -1; }

    @Override
    String code(Throwable t) { return t instanceof AmazonServiceException e ? e.getErrorCode() : null; }

    private PutObjectResult put(String b, String key, byte[] data, ObjectMetadata md) {
        if (md == null) md = new ObjectMetadata();
        md.setContentLength(data.length);
        return s3.putObject(new PutObjectRequest(b, key, new ByteArrayInputStream(data), md));
    }

    private PutObjectResult put(String key, byte[] data) { return put(bucket, key, data, null); }

    private static byte[] read(S3Object o) throws Exception {
        try (o) { return o.getObjectContent().readAllBytes(); }
    }

    private byte[] get(String b, String key) throws Exception { return read(s3.getObject(b, key)); }

    private byte[] get(String key) throws Exception { return get(bucket, key); }

    private List<String> listKeys(String b, String prefix) {
        List<String> keys = new ArrayList<>();
        ListObjectsV2Request req = new ListObjectsV2Request().withBucketName(b).withPrefix(prefix);
        ListObjectsV2Result r;
        do {
            r = s3.listObjectsV2(req);
            r.getObjectSummaries().forEach(o -> keys.add(o.getKey()));
            req.setContinuationToken(r.getNextContinuationToken());
        } while (r.isTruncated());
        return keys;
    }

    private void emptyBucket(String b) {
        List<String> keys = listKeys(b, null);
        for (int i = 0; i < keys.size(); i += 1000) {
            s3.deleteObjects(new DeleteObjectsRequest(b).withKeys(keys.subList(i, Math.min(keys.size(), i + 1000)).toArray(new String[0])));
        }
    }

    private void destroyBucket(String b) {
        try {
            VersionListing vl = s3.listVersions(new ListVersionsRequest().withBucketName(b));
            for (S3VersionSummary v : vl.getVersionSummaries()) s3.deleteVersion(b, v.getKey(), v.getVersionId());
        } catch (Exception ignored) {
        }
        try { emptyBucket(b); } catch (Exception ignored) {}
        try { s3.deleteBucket(b); } catch (Exception ignored) {}
    }

    private String newBucket(String suffix) {
        String b = bucket + "-" + suffix;
        extraBuckets.add(b);
        s3.createBucket(b);
        return b;
    }

    @Override
    void cleanup() {
        for (String b : extraBuckets) destroyBucket(b);
        destroyBucket(bucket);
        s3.shutdown();
    }

    private static List<String> keysOf(List<S3ObjectSummary> l) { return l.stream().map(S3ObjectSummary::getKey).toList(); }

    @Override
    Map<String, Body> scenarios() {
        Map<String, Body> m = new LinkedHashMap<>();
        m.put("create_bucket", () -> s3.createBucket(bucket));
        m.put("head_bucket", () -> s3.headBucket(new HeadBucketRequest(bucket)));
        m.put("head_bucket_missing", () -> expectError("HeadBucket missing",
                () -> s3.headBucket(new HeadBucketRequest("m-missing-" + randLower(12))), Set.of(404), "NoSuchBucket", "NotFound"));
        m.put("list_buckets", () -> {
            List<String> names = s3.listBuckets().stream().map(Bucket::getName).toList();
            U.check(names.contains(bucket), "bucket " + bucket + " not in ListBuckets (" + names.size() + " buckets)");
        });
        m.put("put_get_small", () -> {
            byte[] data = U.utf8("hello world");
            PutObjectResult p = put("small.txt", data);
            S3Object o = s3.getObject(bucket, "small.txt");
            String getEtag = o.getObjectMetadata().getETag();
            U.bytesEq(data, read(o), "GET body");
            U.eq(U.md5hex(data), U.unquote(p.getETag()), "PUT ETag");
            U.eq(U.md5hex(data), U.unquote(getEtag), "GET ETag");
        });
        m.put("put_get_empty", () -> {
            put("empty", new byte[0]);
            S3Object o = s3.getObject(bucket, "empty");
            long cl = o.getObjectMetadata().getContentLength();
            U.eq(0, read(o).length, "body length");
            U.eq(0L, cl, "Content-Length");
        });
        m.put("put_get_large", () -> {
            byte[] data = U.rand(8 * U.MIB);
            put("large.bin", data);
            U.bytesEq(data, get("large.bin"), "GET body");
        });
        m.put("special_keys", () -> {
            List<String> errs = new ArrayList<>();
            for (String k : U.SPECIAL_KEYS) {
                try {
                    byte[] data = U.utf8("key=" + k);
                    put(k, data);
                    U.bytesEq(data, get(k), "GET");
                    List<String> keys = listKeys(bucket, k);
                    U.check(keys.contains(k), "not in List: " + keys);
                } catch (Exception | AssertionError e) {
                    errs.add("[" + k + "] " + U.describe(U.unwrap(e)));
                }
            }
            U.check(errs.isEmpty(), String.join("; ", errs));
        });
        m.put("head_object_metadata", () -> {
            ObjectMetadata md = new ObjectMetadata();
            md.setContentType("text/plain; charset=utf-8");
            md.setCacheControl("max-age=3600");
            md.setContentDisposition("attachment; filename=\"m.txt\"");
            md.setContentEncoding("gzip");
            U.USER_META.forEach(md::addUserMetadata);
            put(bucket, "meta.txt", U.utf8("metadata"), md);
            ObjectMetadata h = s3.getObjectMetadata(bucket, "meta.txt");
            U.eq("text/plain; charset=utf-8", h.getContentType(), "Content-Type");
            U.eq("max-age=3600", h.getCacheControl(), "Cache-Control");
            U.eq("attachment; filename=\"m.txt\"", h.getContentDisposition(), "Content-Disposition");
            U.eq("gzip", h.getContentEncoding(), "Content-Encoding");
            U.eq(U.USER_META, new HashMap<>(h.getUserMetadata()), "user metadata");
        });
        m.put("get_range", () -> {
            byte[] data = U.rand(100);
            put("range.bin", data);
            record R(GetObjectRequest req, int from, String cr) {}
            GetObjectRequest suffix = new GetObjectRequest(bucket, "range.bin");
            suffix.putCustomRequestHeader("Range", "bytes=-5"); // v1 has no typed suffix-range API
            for (R r : List.of(new R(new GetObjectRequest(bucket, "range.bin").withRange(10, 19), 10, "bytes 10-19/100"),
                    new R(suffix, 95, "bytes 95-99/100"),
                    new R(new GetObjectRequest(bucket, "range.bin").withRange(95), 95, "bytes 95-99/100"))) {
                S3Object o = s3.getObject(r.req());
                Object cr = o.getObjectMetadata().getRawMetadataValue("Content-Range");
                byte[] body = read(o);
                int to = r.from() == 10 ? 20 : 100;
                U.bytesEq(Arrays.copyOfRange(data, r.from(), to), body, "range from " + r.from());
                U.eq(r.cr(), cr, "Content-Range");
            }
        });
        m.put("get_range_invalid", () -> {
            put("range-inv.bin", U.rand(100));
            expectError("GET bytes=200-300", () -> read(s3.getObject(new GetObjectRequest(bucket, "range-inv.bin").withRange(200, 300))),
                    Set.of(416), "InvalidRange");
        });
        m.put("get_missing_key", () -> expectError("GET missing", () -> read(s3.getObject(bucket, "no-such-key-" + randLower(8))), Set.of(404), "NoSuchKey"));
        m.put("get_if_none_match", () -> {
            String etag = put("inm.txt", U.utf8("if-none-match")).getETag();
            // v1 maps 304/412 on constrained GETs to a null return value instead of an exception.
            S3Object o = s3.getObject(new GetObjectRequest(bucket, "inm.txt").withNonmatchingETagConstraint(etag));
            if (o != null) { o.close(); throw new AssertionError("expected 304 (null S3Object), got object"); }
        });
        m.put("get_if_match_fail", () -> {
            put("im.txt", U.utf8("if-match"));
            S3Object o = s3.getObject(new GetObjectRequest(bucket, "im.txt").withMatchingETagConstraint("\"wrong\""));
            if (o != null) { o.close(); throw new AssertionError("expected 412 (null S3Object), got object"); }
        });
        m.put("put_if_none_match", () -> {
            put("pinm.txt", U.utf8("first"));
            // v1 has no typed If-None-Match on PutObject; use the generic custom request header API.
            expectError("PUT If-None-Match:* existing", () -> {
                ObjectMetadata md = new ObjectMetadata();
                md.setContentLength(6);
                PutObjectRequest req = new PutObjectRequest(bucket, "pinm.txt", new ByteArrayInputStream(U.utf8("second")), md);
                req.putCustomRequestHeader("If-None-Match", "*");
                s3.putObject(req);
            }, Set.of(412), "PreconditionFailed");
            String fresh = "pinm-new-" + randLower(8);
            ObjectMetadata md = new ObjectMetadata();
            md.setContentLength(3);
            PutObjectRequest req = new PutObjectRequest(bucket, fresh, new ByteArrayInputStream(U.utf8("new")), md);
            req.putCustomRequestHeader("If-None-Match", "*");
            s3.putObject(req);
            U.bytesEq(U.utf8("new"), get(fresh), "new key body");
            U.bytesEq(U.utf8("first"), get("pinm.txt"), "existing key unchanged");
        });
        m.put("delete_object", () -> {
            put("del.txt", U.utf8("x"));
            s3.deleteObject(bucket, "del.txt");
            expectError("HEAD deleted", () -> s3.getObjectMetadata(bucket, "del.txt"), Set.of(404), "NoSuchKey", "NotFound");
        });
        m.put("delete_missing_object", () -> s3.deleteObject(bucket, "never-existed-" + randLower(8)));
        m.put("delete_objects", () -> {
            List<String> keys = List.of("dm/1", "dm/2", "dm/3");
            for (String k : keys) put(k, U.utf8(k));
            DeleteObjectsResult r = s3.deleteObjects(new DeleteObjectsRequest(bucket).withKeys(keys.toArray(new String[0])));
            U.eq(3, r.getDeletedObjects().size(), "deleted count");
            U.eq(List.of(), listKeys(bucket, "dm/"), "keys left after DeleteObjects");
        });
        m.put("list_v2_basic", () -> {
            Map<String, byte[]> objs = new TreeMap<>(Map.of("lb/c", U.rand(3), "lb/a", U.rand(1), "lb/b", U.rand(2)));
            for (var e : objs.entrySet()) put(e.getKey(), e.getValue());
            ListObjectsV2Result r = s3.listObjectsV2(new ListObjectsV2Request().withBucketName(bucket).withPrefix("lb/"));
            U.eq(new ArrayList<>(objs.keySet()), keysOf(r.getObjectSummaries()), "keys");
            for (S3ObjectSummary o : r.getObjectSummaries()) {
                U.eq((long) objs.get(o.getKey()).length, o.getSize(), "Size of " + o.getKey());
                U.eq(U.md5hex(objs.get(o.getKey())), U.unquote(o.getETag()), "ETag of " + o.getKey());
            }
        });
        m.put("list_v2_prefix_delimiter", () -> {
            String b = newBucket("pd");
            for (String k : List.of("a/1", "a/2", "b/1", "c")) put(b, k, U.utf8(k), null);
            ListObjectsV2Result r = s3.listObjectsV2(new ListObjectsV2Request().withBucketName(b).withPrefix("").withDelimiter("/"));
            U.eq(List.of("a/", "b/"), r.getCommonPrefixes(), "CommonPrefixes");
            U.eq(List.of("c"), keysOf(r.getObjectSummaries()), "Contents");
        });
        m.put("list_v2_pagination", () -> {
            List<String> expected = new ArrayList<>();
            for (int i = 0; i < 25; i++) {
                String k = String.format("pg/k%02d", i);
                expected.add(k);
                put(k, U.utf8(k));
            }
            List<String> got = new ArrayList<>();
            int pages = 0;
            ListObjectsV2Request req = new ListObjectsV2Request().withBucketName(bucket).withPrefix("pg/").withMaxKeys(10);
            ListObjectsV2Result r;
            do {
                r = s3.listObjectsV2(req);
                pages++;
                U.check(r.getObjectSummaries().size() <= 10, "page " + pages + " has " + r.getObjectSummaries().size() + " keys");
                got.addAll(keysOf(r.getObjectSummaries()));
                req.setContinuationToken(r.getNextContinuationToken());
                U.check(pages <= 10, "too many pages");
            } while (r.isTruncated());
            U.eq(3, pages, "page count");
            U.eq(expected, got, "keys across pages");
        });
        m.put("list_v2_start_after", () -> {
            for (int i = 0; i < 5; i++) put("sa/k" + i, U.utf8("x"));
            ListObjectsV2Result r = s3.listObjectsV2(new ListObjectsV2Request().withBucketName(bucket).withPrefix("sa/").withStartAfter("sa/k2"));
            U.eq(List.of("sa/k3", "sa/k4"), keysOf(r.getObjectSummaries()), "keys after sa/k2");
        });
        m.put("list_v1", () -> {
            for (int i = 0; i < 5; i++) put("l1/k" + i, U.utf8("x"));
            ObjectListing r = s3.listObjects(new ListObjectsRequest().withBucketName(bucket).withPrefix("l1/").withMarker("l1/k1"));
            U.eq(List.of("l1/k2", "l1/k3", "l1/k4"), keysOf(r.getObjectSummaries()), "keys after marker l1/k1");
            ObjectListing page = s3.listObjects(new ListObjectsRequest().withBucketName(bucket).withPrefix("l1/").withMaxKeys(2));
            U.eq(List.of("l1/k0", "l1/k1"), keysOf(page.getObjectSummaries()), "first page MaxKeys=2");
            U.eq(true, page.isTruncated(), "IsTruncated");
        });
        m.put("copy_object", () -> {
            byte[] data = U.utf8("copy me");
            ObjectMetadata md = new ObjectMetadata();
            md.setContentType("text/x-copy");
            md.addUserMetadata("color", "Blue");
            put(bucket, "cp/src", data, md);
            s3.copyObject(bucket, "cp/src", bucket, "cp/dst");
            S3Object o = s3.getObject(bucket, "cp/dst");
            ObjectMetadata h = o.getObjectMetadata();
            U.bytesEq(data, read(o), "copied body");
            U.eq(Map.of("color", "Blue"), new HashMap<>(h.getUserMetadata()), "copied metadata");
            U.eq("text/x-copy", h.getContentType(), "copied Content-Type");
        });
        m.put("copy_object_replace_metadata", () -> {
            byte[] data = U.utf8("copy me");
            ObjectMetadata md = new ObjectMetadata();
            md.addUserMetadata("color", "Blue");
            md.addUserMetadata("old", "1");
            put(bucket, "cpr/src", data, md);
            ObjectMetadata nm = new ObjectMetadata();
            nm.setContentType("text/x-replaced");
            nm.addUserMetadata("color", "Red");
            nm.addUserMetadata("new", "2");
            s3.copyObject(new CopyObjectRequest(bucket, "cpr/src", bucket, "cpr/dst").withNewObjectMetadata(nm)); // => REPLACE
            ObjectMetadata h = s3.getObjectMetadata(bucket, "cpr/dst");
            U.eq(Map.of("color", "Red", "new", "2"), new HashMap<>(h.getUserMetadata()), "replaced metadata");
            U.eq("text/x-replaced", h.getContentType(), "replaced Content-Type");
            U.bytesEq(data, get("cpr/dst"), "copied body");
        });
        m.put("multipart_upload", () -> {
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(5 * U.MIB), U.rand(1024)};
            String key = "mpu/obj";
            String id = s3.initiateMultipartUpload(new InitiateMultipartUploadRequest(bucket, key)).getUploadId();
            List<PartETag> etags = new ArrayList<>();
            for (int i = 0; i < parts.length; i++) etags.add(uploadPart(key, id, i + 1, parts[i]).getPartETag());
            CompleteMultipartUploadResult c = s3.completeMultipartUpload(new CompleteMultipartUploadRequest(bucket, key, id, etags));
            String etag = U.unquote(c.getETag());
            U.check(etag != null && etag.matches("[0-9a-f]{32}-3"), "multipart ETag not <hex>-3: " + c.getETag());
            U.bytesEq(V2Suite.concat(parts), get(key), "GET body");
        });
        m.put("multipart_abort", () -> {
            String key = "mpu/abort";
            String id = s3.initiateMultipartUpload(new InitiateMultipartUploadRequest(bucket, key)).getUploadId();
            uploadPart(key, id, 1, U.rand(1024));
            s3.abortMultipartUpload(new AbortMultipartUploadRequest(bucket, key, id));
            expectError("ListParts after abort", () -> s3.listParts(new ListPartsRequest(bucket, key, id)), Set.of(404), "NoSuchUpload");
        });
        m.put("multipart_list_parts", () -> {
            String key = "mpu/list";
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(1024)};
            String id = s3.initiateMultipartUpload(new InitiateMultipartUploadRequest(bucket, key)).getUploadId();
            try {
                for (int i = 0; i < parts.length; i++) uploadPart(key, id, i + 1, parts[i]);
                PartListing r = s3.listParts(new ListPartsRequest(bucket, key, id));
                U.eq(2, r.getParts().size(), "part count");
                for (int i = 0; i < 2; i++) {
                    PartSummary p = r.getParts().get(i);
                    U.eq(i + 1, p.getPartNumber(), "PartNumber");
                    U.eq((long) parts[i].length, p.getSize(), "Size of part " + (i + 1));
                    U.eq(U.md5hex(parts[i]), U.unquote(p.getETag()), "ETag of part " + (i + 1));
                }
            } finally {
                try { s3.abortMultipartUpload(new AbortMultipartUploadRequest(bucket, key, id)); } catch (Exception ignored) {}
            }
        });
        m.put("multipart_too_small", () -> {
            String key = "mpu/small";
            String id = s3.initiateMultipartUpload(new InitiateMultipartUploadRequest(bucket, key)).getUploadId();
            try {
                List<PartETag> etags = new ArrayList<>();
                for (int i = 1; i <= 2; i++) etags.add(uploadPart(key, id, i, U.rand(1024)).getPartETag());
                expectError("Complete with 1 KiB parts", () -> s3.completeMultipartUpload(new CompleteMultipartUploadRequest(bucket, key, id, etags)),
                        Set.of(), "EntityTooSmall");
            } finally {
                try { s3.abortMultipartUpload(new AbortMultipartUploadRequest(bucket, key, id)); } catch (Exception ignored) {}
            }
        });
        m.put("high_level_upload", () -> {
            byte[] data = U.rand(20 * U.MIB);
            TransferManager tm = TransferManagerBuilder.standard().withS3Client(s3).withMultipartUploadThreshold(8L * U.MIB)
                    .withMinimumUploadPartSize(5L * U.MIB).withShutDownThreadPools(true).build();
            File tmp = File.createTempFile("matrix-v1-", ".bin");
            try {
                ObjectMetadata md = new ObjectMetadata();
                md.setContentLength(data.length);
                tm.upload(bucket, "tm/obj", new ByteArrayInputStream(data), md).waitForCompletion();
                tm.download(bucket, "tm/obj", tmp).waitForCompletion();
                U.bytesEq(data, Files.readAllBytes(tmp.toPath()), "downloaded body");
            } finally {
                tm.shutdownNow(false);
                tmp.delete();
            }
        });
        m.put("presigned_get", () -> {
            byte[] data = U.utf8("presigned get body");
            put("ps/get", data);
            String url = s3.generatePresignedUrl(bucket, "ps/get", new Date(System.currentTimeMillis() + 600_000), HttpMethod.GET).toString();
            U.bytesEq(data, U.httpGet(url), "presigned GET body");
        });
        m.put("presigned_put", () -> {
            byte[] data = U.utf8("presigned put body");
            String url = s3.generatePresignedUrl(bucket, "ps/put", new Date(System.currentTimeMillis() + 600_000), HttpMethod.PUT).toString();
            U.httpPut(url, data);
            U.bytesEq(data, get("ps/put"), "SDK GET after presigned PUT");
        });
        for (String alg : U.CHECKSUM_ALGS) {
            m.put("checksum_" + alg.toLowerCase(Locale.ROOT), () -> {
                throw new Skip("aws-sdk-java v1 has no flexible checksum API (ChecksumAlgorithm / x-amz-checksum-*)");
            });
        }
        m.put("content_md5_bad", () -> {
            ObjectMetadata md = new ObjectMetadata();
            md.setContentMD5(U.md5b64(U.utf8("something else")));
            expectError("PUT bad Content-MD5", () -> put(bucket, "badmd5", U.utf8("payload"), md), Set.of(400), "BadDigest", "InvalidDigest");
        });
        m.put("bad_credentials", () -> {
            AmazonS3 bad = client("wrong-secret-key");
            try {
                expectError("ListBuckets with bad secret", bad::listBuckets, Set.of(403), "SignatureDoesNotMatch", "InvalidAccessKeyId");
            } finally {
                bad.shutdown();
            }
        });
        m.put("delete_bucket_not_empty", () -> {
            put("nonempty/x", U.utf8("x"));
            expectError("DeleteBucket non-empty", () -> s3.deleteBucket(bucket), Set.of(409), "BucketNotEmpty");
        });
        m.put("versioning", () -> {
            String b = newBucket("ver");
            s3.setBucketVersioningConfiguration(new SetBucketVersioningConfigurationRequest(b,
                    new BucketVersioningConfiguration(BucketVersioningConfiguration.ENABLED)));
            String v1 = put(b, "doc", U.utf8("version-1"), null).getVersionId();
            String v2 = put(b, "doc", U.utf8("version-2"), null).getVersionId();
            U.check(v1 != null && v2 != null && !v1.equals(v2), "PUT returned version ids " + v1 + ", " + v2);
            List<S3VersionSummary> vs = s3.listVersions(new ListVersionsRequest().withBucketName(b).withPrefix("doc")).getVersionSummaries();
            U.eq(2, vs.size(), "version count");
            U.eq(Set.of(v1, v2), new HashSet<>(vs.stream().map(S3VersionSummary::getVersionId).toList()), "listed version ids");
            U.bytesEq(U.utf8("version-1"), read(s3.getObject(new GetObjectRequest(b, "doc", v1))), "GET old version");
            U.bytesEq(U.utf8("version-2"), get(b, "doc"), "GET latest");
        });
        m.put("object_tagging", () -> {
            put("tag.txt", U.utf8("tagged"));
            Map<String, String> tags = Map.of("project", "matrix", "env", "test");
            s3.setObjectTagging(new SetObjectTaggingRequest(bucket, "tag.txt",
                    new ObjectTagging(tags.entrySet().stream().map(e -> new Tag(e.getKey(), e.getValue())).toList())));
            Map<String, String> got = new HashMap<>();
            s3.getObjectTagging(new GetObjectTaggingRequest(bucket, "tag.txt")).getTagSet().forEach(t -> got.put(t.getKey(), t.getValue()));
            U.eq(tags, got, "tags");
        });
        m.put("delete_bucket", () -> {
            emptyBucket(bucket);
            s3.deleteBucket(bucket);
            expectError("HeadBucket after delete", () -> s3.headBucket(new HeadBucketRequest(bucket)), Set.of(404), "NoSuchBucket", "NotFound");
        });
        return m;
    }

    private UploadPartResult uploadPart(String key, String id, int n, byte[] data) {
        return s3.uploadPart(new UploadPartRequest().withBucketName(bucket).withKey(key).withUploadId(id).withPartNumber(n)
                .withInputStream(new ByteArrayInputStream(data)).withPartSize(data.length));
    }
}
