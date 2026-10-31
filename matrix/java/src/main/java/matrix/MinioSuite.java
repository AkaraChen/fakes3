package matrix;

import io.minio.*;
import io.minio.errors.ErrorResponseException;
import io.minio.messages.*;

import java.io.ByteArrayInputStream;
import java.util.*;
import java.util.concurrent.TimeUnit;

/** minio-java: MinioClient (high level) + MinioAsyncClient (low-level S3 APIs such as multipart). */
final class MinioSuite extends Suite {
    private final MinioClient mc;
    private final MinioAsyncClient ac;
    private final List<String> extraBuckets = Collections.synchronizedList(new ArrayList<>());

    MinioSuite(Env env, String sdk) {
        super(env, sdk);
        mc = MinioClient.builder().endpoint(env.endpoint()).region(env.region()).credentials(env.accessKey(), env.secretKey()).build();
        ac = MinioAsyncClient.builder().endpoint(env.endpoint()).region(env.region()).credentials(env.accessKey(), env.secretKey()).build();
    }

    @Override
    int status(Throwable t) {
        if (t instanceof ErrorResponseException e && e.response() != null) return e.response().code();
        if (t instanceof io.minio.errors.ServerException e) return e.statusCode(); // e.g. body-less 304
        return -1;
    }

    @Override
    String code(Throwable t) {
        return t instanceof ErrorResponseException e && e.errorResponse() != null ? e.errorResponse().code() : null;
    }

    private ObjectWriteResponse put(String b, String key, byte[] data) throws Exception {
        return mc.putObject(PutObjectArgs.builder().bucket(b).object(key).data(data, data.length).build());
    }

    private ObjectWriteResponse put(String key, byte[] data) throws Exception { return put(bucket, key, data); }

    private byte[] get(String b, String key) throws Exception {
        try (GetObjectResponse r = mc.getObject(GetObjectArgs.builder().bucket(b).object(key).build())) {
            return r.readAllBytes();
        }
    }

    private byte[] get(String key) throws Exception { return get(bucket, key); }

    private List<String> listKeys(String b, String prefix) throws Exception {
        List<String> keys = new ArrayList<>();
        for (Result<Item> r : mc.listObjects(ListObjectsArgs.builder().bucket(b).prefix(prefix).recursive(true).build())) keys.add(r.get().objectName());
        return keys;
    }

    private void emptyBucket(String b, boolean versions) throws Exception {
        List<DeleteRequest.Object> objs = new ArrayList<>();
        for (Result<Item> r : mc.listObjects(ListObjectsArgs.builder().bucket(b).recursive(true).includeVersions(versions).build())) {
            Item it = r.get();
            objs.add(new DeleteRequest.Object(it.objectName(), it.versionId()));
        }
        for (int i = 0; i < objs.size(); i += 1000) {
            ac.deleteObjects(DeleteObjectsArgs.builder().bucket(b).objects(objs.subList(i, Math.min(objs.size(), i + 1000))).build()).get();
        }
    }

    private void destroyBucket(String b) {
        try { emptyBucket(b, true); } catch (Exception ignored) {}
        try { emptyBucket(b, false); } catch (Exception ignored) {}
        try { mc.removeBucket(RemoveBucketArgs.builder().bucket(b).build()); } catch (Exception ignored) {}
    }

    private String newBucket(String suffix) throws Exception {
        String b = bucket + "-" + suffix;
        extraBuckets.add(b);
        mc.makeBucket(MakeBucketArgs.builder().bucket(b).build());
        return b;
    }

    @Override
    void cleanup() throws Exception {
        for (String b : extraBuckets) destroyBucket(b);
        destroyBucket(bucket);
        mc.close();
        ac.close();
    }

    private static List<String> names(List<? extends Item> l) { return l.stream().map(Item::objectName).toList(); }

    private String createUpload(String key) throws Exception {
        return ac.createMultipartUpload(CreateMultipartUploadArgs.builder().bucket(bucket).object(key).build()).get().result().uploadId();
    }

    private Part uploadPart(String key, String id, int n, byte[] data) throws Exception {
        return ac.uploadPart(UploadPartArgs.builder().bucket(bucket).object(key).uploadId(id).partNumber(n).data(data, data.length).build()).get().part();
    }

    private void abort(String key, String id) {
        try { ac.abortMultipartUpload(AbortMultipartUploadArgs.builder().bucket(bucket).object(key).uploadId(id).build()).get(); } catch (Exception ignored) {}
    }

    private static Map<String, String> meta(Http.Headers h) {
        Map<String, String> m = new HashMap<>();
        for (Map.Entry<String, String> e : h.entries()) m.put(e.getKey().toLowerCase(Locale.ROOT), e.getValue());
        return m;
    }

    @Override
    Map<String, Body> scenarios() {
        Map<String, Body> m = new LinkedHashMap<>();
        m.put("create_bucket", () -> mc.makeBucket(MakeBucketArgs.builder().bucket(bucket).build()));
        m.put("head_bucket", () -> U.check(mc.bucketExists(BucketExistsArgs.builder().bucket(bucket).build()), "bucketExists returned false"));
        m.put("head_bucket_missing", () -> expectError("HeadBucket missing",
                () -> ac.headBucket(HeadBucketArgs.builder().bucket("m-missing-" + randLower(12)).build()).get(), Set.of(404), "NoSuchBucket", "NotFound"));
        m.put("list_buckets", () -> {
            List<String> names = mc.listBuckets().stream().map(ListAllMyBucketsResult.Bucket::name).toList();
            U.check(names.contains(bucket), "bucket " + bucket + " not in ListBuckets (" + names.size() + " buckets)");
        });
        m.put("put_get_small", () -> {
            byte[] data = U.utf8("hello world");
            ObjectWriteResponse p = put("small.txt", data);
            try (GetObjectResponse g = mc.getObject(GetObjectArgs.builder().bucket(bucket).object("small.txt").build())) {
                U.bytesEq(data, g.readAllBytes(), "GET body");
                U.eq(U.md5hex(data), U.unquote(g.headers().get("ETag")), "GET ETag");
            }
            U.eq(U.md5hex(data), U.unquote(p.etag()), "PUT ETag");
        });
        m.put("put_get_empty", () -> {
            put("empty", new byte[0]);
            try (GetObjectResponse g = mc.getObject(GetObjectArgs.builder().bucket(bucket).object("empty").build())) {
                U.eq(0, g.readAllBytes().length, "body length");
                U.eq("0", g.headers().get("Content-Length"), "Content-Length");
            }
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
            byte[] data = U.utf8("metadata");
            mc.putObject(PutObjectArgs.builder().bucket(bucket).object("meta.txt").data(data, data.length)
                    .contentType("text/plain; charset=utf-8")
                    .headers(Map.of("Cache-Control", "max-age=3600", "Content-Disposition", "attachment; filename=\"m.txt\"", "Content-Encoding", "gzip"))
                    .userMetadata(U.USER_META).build());
            StatObjectResponse h = mc.statObject(StatObjectArgs.builder().bucket(bucket).object("meta.txt").build());
            U.eq("text/plain; charset=utf-8", h.contentType(), "Content-Type");
            U.eq("max-age=3600", h.headers().get("Cache-Control"), "Cache-Control");
            U.eq("attachment; filename=\"m.txt\"", h.headers().get("Content-Disposition"), "Content-Disposition");
            U.eq("gzip", h.headers().get("Content-Encoding"), "Content-Encoding");
            U.eq(U.USER_META, meta(h.userMetadata()), "user metadata");
        });
        m.put("get_range", () -> {
            byte[] data = U.rand(100);
            put("range.bin", data);
            record R(GetObjectArgs args, int from, int to, String cr) {}
            List<R> cases = List.of(
                    new R(GetObjectArgs.builder().bucket(bucket).object("range.bin").offset(10L).length(10L).build(), 10, 20, "bytes 10-19/100"),
                    // minio-java has no typed suffix-range API; send the Range header via extraHeaders.
                    new R(GetObjectArgs.builder().bucket(bucket).object("range.bin").extraHeaders(Map.of("Range", "bytes=-5")).build(), 95, 100, "bytes 95-99/100"),
                    new R(GetObjectArgs.builder().bucket(bucket).object("range.bin").offset(95L).build(), 95, 100, "bytes 95-99/100"));
            for (R r : cases) {
                try (GetObjectResponse g = mc.getObject(r.args())) {
                    U.bytesEq(Arrays.copyOfRange(data, r.from(), r.to()), g.readAllBytes(), "range " + r.cr());
                    U.eq(r.cr(), g.headers().get("Content-Range"), "Content-Range");
                }
            }
        });
        m.put("get_range_invalid", () -> {
            put("range-inv.bin", U.rand(100));
            expectError("GET bytes=200-300", () -> get(GetObjectArgs.builder().bucket(bucket).object("range-inv.bin").offset(200L).length(101L).build()),
                    Set.of(416), "InvalidRange");
        });
        m.put("get_missing_key", () -> expectError("GET missing", () -> get("no-such-key-" + randLower(8)), Set.of(404), "NoSuchKey"));
        m.put("get_if_none_match", () -> {
            String etag = put("inm.txt", U.utf8("if-none-match")).etag();
            expectError("GET If-None-Match", () -> get(GetObjectArgs.builder().bucket(bucket).object("inm.txt").notMatchETag(etag).build()),
                    Set.of(304), "NotModified");
        });
        m.put("get_if_match_fail", () -> {
            put("im.txt", U.utf8("if-match"));
            expectError("GET If-Match wrong", () -> get(GetObjectArgs.builder().bucket(bucket).object("im.txt").matchETag("\"wrong\"").build()),
                    Set.of(412), "PreconditionFailed");
        });
        m.put("put_if_none_match", () -> {
            put("pinm.txt", U.utf8("first"));
            byte[] second = U.utf8("second");
            expectError("PUT If-None-Match:* existing", () -> mc.putObject(PutObjectArgs.builder().bucket(bucket).object("pinm.txt")
                    .data(second, second.length).headers(Map.of("If-None-Match", "*")).build()), Set.of(412), "PreconditionFailed");
            String fresh = "pinm-new-" + randLower(8);
            byte[] nw = U.utf8("new");
            mc.putObject(PutObjectArgs.builder().bucket(bucket).object(fresh).data(nw, nw.length).headers(Map.of("If-None-Match", "*")).build());
            U.bytesEq(nw, get(fresh), "new key body");
            U.bytesEq(U.utf8("first"), get("pinm.txt"), "existing key unchanged");
        });
        m.put("delete_object", () -> {
            put("del.txt", U.utf8("x"));
            mc.removeObject(RemoveObjectArgs.builder().bucket(bucket).object("del.txt").build());
            expectError("HEAD deleted", () -> mc.statObject(StatObjectArgs.builder().bucket(bucket).object("del.txt").build()),
                    Set.of(404), "NoSuchKey", "NotFound");
        });
        m.put("delete_missing_object", () -> mc.removeObject(RemoveObjectArgs.builder().bucket(bucket).object("never-existed-" + randLower(8)).build()));
        m.put("delete_objects", () -> {
            List<String> keys = List.of("dm/1", "dm/2", "dm/3");
            for (String k : keys) put(k, U.utf8(k));
            DeleteResult r = ac.deleteObjects(DeleteObjectsArgs.builder().bucket(bucket)
                    .objects(keys.stream().map(DeleteRequest.Object::new).toList()).build()).get().result();
            U.check(r.errors().isEmpty(), "DeleteObjects errors: " + r.errors());
            U.eq(List.of(), listKeys(bucket, "dm/"), "keys left after DeleteObjects");
        });
        m.put("list_v2_basic", () -> {
            Map<String, byte[]> objs = new TreeMap<>(Map.of("lb/c", U.rand(3), "lb/a", U.rand(1), "lb/b", U.rand(2)));
            for (var e : objs.entrySet()) put(e.getKey(), e.getValue());
            ListBucketResultV2 r = ac.listObjectsV2(ListObjectsV2Args.builder().bucket(bucket).prefix("lb/").build()).get().result();
            U.eq(new ArrayList<>(objs.keySet()), names(r.contents()), "keys");
            for (Item o : r.contents()) {
                U.eq((long) objs.get(o.objectName()).length, o.size(), "Size of " + o.objectName());
                U.eq(U.md5hex(objs.get(o.objectName())), U.unquote(o.etag()), "ETag of " + o.objectName());
            }
        });
        m.put("list_v2_prefix_delimiter", () -> {
            String b = newBucket("pd");
            for (String k : List.of("a/1", "a/2", "b/1", "c")) put(b, k, U.utf8(k));
            ListBucketResultV2 r = ac.listObjectsV2(ListObjectsV2Args.builder().bucket(b).prefix("").delimiter("/").build()).get().result();
            U.eq(List.of("a/", "b/"), r.commonPrefixes().stream().map(p -> p.toItem().objectName()).toList(), "CommonPrefixes");
            U.eq(List.of("c"), names(r.contents()), "Contents");
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
            String token = null;
            do {
                ListObjectsV2Args.Builder a = ListObjectsV2Args.builder().bucket(bucket).prefix("pg/").maxKeys(10);
                if (token != null) a.continuationToken(token);
                ListBucketResultV2 r = ac.listObjectsV2(a.build()).get().result();
                pages++;
                U.check(r.contents().size() <= 10, "page " + pages + " has " + r.contents().size() + " keys");
                got.addAll(names(r.contents()));
                token = r.isTruncated() ? r.nextContinuationToken() : null;
                U.check(pages <= 10, "too many pages");
            } while (token != null);
            U.eq(3, pages, "page count");
            U.eq(expected, got, "keys across pages");
        });
        m.put("list_v2_start_after", () -> {
            for (int i = 0; i < 5; i++) put("sa/k" + i, U.utf8("x"));
            ListBucketResultV2 r = ac.listObjectsV2(ListObjectsV2Args.builder().bucket(bucket).prefix("sa/").startAfter("sa/k2").build()).get().result();
            U.eq(List.of("sa/k3", "sa/k4"), names(r.contents()), "keys after sa/k2");
        });
        m.put("list_v1", () -> {
            for (int i = 0; i < 5; i++) put("l1/k" + i, U.utf8("x"));
            ListBucketResultV1 r = ac.listObjectsV1(ListObjectsV1Args.builder().bucket(bucket).prefix("l1/").marker("l1/k1").build()).get().result();
            U.eq(List.of("l1/k2", "l1/k3", "l1/k4"), names(r.contents()), "keys after marker l1/k1");
            ListBucketResultV1 page = ac.listObjectsV1(ListObjectsV1Args.builder().bucket(bucket).prefix("l1/").maxKeys(2).build()).get().result();
            U.eq(List.of("l1/k0", "l1/k1"), names(page.contents()), "first page MaxKeys=2");
            U.eq(true, page.isTruncated(), "IsTruncated");
        });
        m.put("copy_object", () -> {
            byte[] data = U.utf8("copy me");
            mc.putObject(PutObjectArgs.builder().bucket(bucket).object("cp/src").data(data, data.length).contentType("text/x-copy")
                    .userMetadata(Map.of("color", "Blue")).build());
            mc.copyObject(CopyObjectArgs.builder().bucket(bucket).object("cp/dst")
                    .source(SourceObject.builder().bucket(bucket).object("cp/src").build()).build());
            try (GetObjectResponse g = mc.getObject(GetObjectArgs.builder().bucket(bucket).object("cp/dst").build())) {
                U.bytesEq(data, g.readAllBytes(), "copied body");
                U.eq("Blue", g.headers().get("x-amz-meta-color"), "copied metadata color");
                U.eq("text/x-copy", g.headers().get("Content-Type"), "copied Content-Type");
            }
        });
        m.put("copy_object_replace_metadata", () -> {
            byte[] data = U.utf8("copy me");
            mc.putObject(PutObjectArgs.builder().bucket(bucket).object("cpr/src").data(data, data.length)
                    .userMetadata(Map.of("color", "Blue", "old", "1")).build());
            mc.copyObject(CopyObjectArgs.builder().bucket(bucket).object("cpr/dst")
                    .source(SourceObject.builder().bucket(bucket).object("cpr/src").build())
                    .metadataDirective(Directive.REPLACE).userMetadata(Map.of("color", "Red", "new", "2"))
                    .headers(Map.of("Content-Type", "text/x-replaced")).build());
            StatObjectResponse h = mc.statObject(StatObjectArgs.builder().bucket(bucket).object("cpr/dst").build());
            U.eq(Map.of("color", "Red", "new", "2"), meta(h.userMetadata()), "replaced metadata");
            U.eq("text/x-replaced", h.contentType(), "replaced Content-Type");
            U.bytesEq(data, get("cpr/dst"), "copied body");
        });
        m.put("multipart_upload", () -> {
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(5 * U.MIB), U.rand(1024)};
            String key = "mpu/obj";
            String id = createUpload(key);
            Part[] done = new Part[parts.length];
            for (int i = 0; i < parts.length; i++) done[i] = uploadPart(key, id, i + 1, parts[i]);
            ObjectWriteResponse c = ac.completeMultipartUpload(CompleteMultipartUploadArgs.builder().bucket(bucket).object(key).uploadId(id).parts(done).build()).get();
            String etag = U.unquote(c.etag());
            U.check(etag != null && etag.matches("[0-9a-f]{32}-3"), "multipart ETag not <hex>-3: " + c.etag());
            U.bytesEq(V2Suite.concat(parts), get(key), "GET body");
        });
        m.put("multipart_abort", () -> {
            String key = "mpu/abort";
            String id = createUpload(key);
            uploadPart(key, id, 1, U.rand(1024));
            ac.abortMultipartUpload(AbortMultipartUploadArgs.builder().bucket(bucket).object(key).uploadId(id).build()).get();
            expectError("ListParts after abort", () -> ac.listParts(ListPartsArgs.builder().bucket(bucket).object(key).uploadId(id).build()).get(),
                    Set.of(404), "NoSuchUpload");
        });
        m.put("multipart_list_parts", () -> {
            String key = "mpu/list";
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(1024)};
            String id = createUpload(key);
            try {
                for (int i = 0; i < parts.length; i++) uploadPart(key, id, i + 1, parts[i]);
                List<Part> ps = ac.listParts(ListPartsArgs.builder().bucket(bucket).object(key).uploadId(id).build()).get().result().parts();
                U.eq(2, ps.size(), "part count");
                for (int i = 0; i < 2; i++) {
                    Part p = ps.get(i);
                    U.eq(i + 1, p.partNumber(), "PartNumber");
                    U.eq((long) parts[i].length, p.partSize(), "Size of part " + (i + 1));
                    U.eq(U.md5hex(parts[i]), U.unquote(p.etag()), "ETag of part " + (i + 1));
                }
            } finally {
                abort(key, id);
            }
        });
        m.put("multipart_too_small", () -> {
            String key = "mpu/small";
            String id = createUpload(key);
            try {
                Part[] done = {uploadPart(key, id, 1, U.rand(1024)), uploadPart(key, id, 2, U.rand(1024))};
                expectError("Complete with 1 KiB parts", () -> ac.completeMultipartUpload(CompleteMultipartUploadArgs.builder().bucket(bucket).object(key)
                        .uploadId(id).parts(done).build()).get(), Set.of(), "EntityTooSmall");
            } finally {
                abort(key, id);
            }
        });
        m.put("high_level_upload", () -> {
            // putObject with a stream + part size is minio-java's high-level (auto multipart) upload API.
            byte[] data = U.rand(20 * U.MIB);
            mc.putObject(PutObjectArgs.builder().bucket(bucket).object("tm/obj")
                    .stream(new ByteArrayInputStream(data), (long) data.length, 5L * U.MIB).build());
            U.bytesEq(data, get("tm/obj"), "downloaded body");
        });
        m.put("presigned_get", () -> {
            byte[] data = U.utf8("presigned get body");
            put("ps/get", data);
            String url = mc.getPresignedObjectUrl(GetPresignedObjectUrlArgs.builder().method(Http.Method.GET).bucket(bucket).object("ps/get")
                    .expiry(10, TimeUnit.MINUTES).build());
            U.bytesEq(data, U.httpGet(url), "presigned GET body");
        });
        m.put("presigned_put", () -> {
            byte[] data = U.utf8("presigned put body");
            String url = mc.getPresignedObjectUrl(GetPresignedObjectUrlArgs.builder().method(Http.Method.PUT).bucket(bucket).object("ps/put")
                    .expiry(10, TimeUnit.MINUTES).build());
            U.httpPut(url, data);
            U.bytesEq(data, get("ps/put"), "SDK GET after presigned PUT");
        });
        for (String alg : U.CHECKSUM_ALGS) {
            m.put("checksum_" + alg.toLowerCase(Locale.ROOT), () -> {
                byte[] data = U.rand(64 * 1024 + 7);
                String key = "ck/" + alg;
                String expected = U.checksum(alg, data);
                io.minio.Checksum.Algorithm a = io.minio.Checksum.Algorithm.valueOf(alg);
                try {
                    mc.putObject(PutObjectArgs.builder().bucket(bucket).object(key).data(data, data.length).checksum(a).build());
                } catch (IllegalArgumentException e) {
                    // minio-java's high-level putObject rejects SHA1/SHA256 for single-part uploads
                    // ("unsupported checksum sha1 for part count 1"); fall back to the low-level PutObject API
                    // with headers built by the SDK's own io.minio.Checksum helpers.
                    var hashers = io.minio.Checksum.newHasherMap(new io.minio.Checksum.Algorithm[]{a});
                    io.minio.Checksum.update(hashers, data, data.length);
                    ac.putObject(PutObjectAPIArgs.builder().bucket(bucket).object(key).data(data, data.length)
                            .headers(io.minio.Checksum.makeHeaders(hashers, false, false)).build()).get();
                }
                try (GetObjectResponse g = mc.getObject(GetObjectArgs.builder().bucket(bucket).object(key).fetchChecksum(true).build())) {
                    U.bytesEq(data, g.readAllBytes(), "GET body");
                    String h = "x-amz-checksum-" + alg.toLowerCase(Locale.ROOT);
                    U.eq(expected, g.headers().get(h), h);
                }
            });
        }
        m.put("content_md5_bad", () -> {
            byte[] data = U.utf8("payload");
            expectError("PUT bad Content-MD5", () -> mc.putObject(PutObjectArgs.builder().bucket(bucket).object("badmd5").data(data, data.length)
                    .headers(Map.of("Content-MD5", U.md5b64(U.utf8("something else")))).build()), Set.of(400), "BadDigest", "InvalidDigest");
        });
        m.put("bad_credentials", () -> {
            MinioClient bad = MinioClient.builder().endpoint(env.endpoint()).region(env.region()).credentials(env.accessKey(), "wrong-secret-key").build();
            try {
                expectError("ListBuckets with bad secret", bad::listBuckets, Set.of(403), "SignatureDoesNotMatch", "InvalidAccessKeyId");
            } finally {
                bad.close();
            }
        });
        m.put("delete_bucket_not_empty", () -> {
            put("nonempty/x", U.utf8("x"));
            expectError("DeleteBucket non-empty", () -> mc.removeBucket(RemoveBucketArgs.builder().bucket(bucket).build()), Set.of(409), "BucketNotEmpty");
        });
        m.put("versioning", () -> {
            String b = newBucket("ver");
            mc.setBucketVersioning(SetBucketVersioningArgs.builder().bucket(b)
                    .config(new VersioningConfiguration(VersioningConfiguration.Status.ENABLED, null, null, null)).build());
            String v1 = put(b, "doc", U.utf8("version-1")).versionId();
            String v2 = put(b, "doc", U.utf8("version-2")).versionId();
            U.check(v1 != null && v2 != null && !v1.equals(v2), "PUT returned version ids " + v1 + ", " + v2);
            List<ListVersionsResult.Version> vs = ac.listObjectVersions(ListObjectVersionsArgs.builder().bucket(b).prefix("doc").build()).get().result().contents();
            U.eq(2, vs.size(), "version count");
            U.eq(Set.of(v1, v2), new HashSet<>(vs.stream().map(Item::versionId).toList()), "listed version ids");
            try (GetObjectResponse g = mc.getObject(GetObjectArgs.builder().bucket(b).object("doc").versionId(v1).build())) {
                U.bytesEq(U.utf8("version-1"), g.readAllBytes(), "GET old version");
            }
            U.bytesEq(U.utf8("version-2"), get(b, "doc"), "GET latest");
        });
        m.put("object_tagging", () -> {
            put("tag.txt", U.utf8("tagged"));
            Map<String, String> tags = Map.of("project", "matrix", "env", "test");
            mc.setObjectTags(SetObjectTagsArgs.builder().bucket(bucket).object("tag.txt").tags(tags).build());
            U.eq(tags, new HashMap<>(mc.getObjectTags(GetObjectTagsArgs.builder().bucket(bucket).object("tag.txt").build()).get()), "tags");
        });
        m.put("delete_bucket", () -> {
            emptyBucket(bucket, false);
            mc.removeBucket(RemoveBucketArgs.builder().bucket(bucket).build());
            expectError("HeadBucket after delete", () -> ac.headBucket(HeadBucketArgs.builder().bucket(bucket).build()).get(),
                    Set.of(404), "NoSuchBucket", "NotFound");
        });
        return m;
    }

    private byte[] get(GetObjectArgs a) throws Exception {
        try (GetObjectResponse r = mc.getObject(a)) { return r.readAllBytes(); }
    }
}
