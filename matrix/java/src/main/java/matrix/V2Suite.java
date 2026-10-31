package matrix;

import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.awscore.exception.AwsServiceException;
import software.amazon.awssdk.core.ResponseBytes;
import software.amazon.awssdk.core.async.AsyncRequestBody;
import software.amazon.awssdk.core.async.AsyncResponseTransformer;
import software.amazon.awssdk.core.exception.SdkServiceException;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3AsyncClient;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.S3Configuration;
import software.amazon.awssdk.services.s3.model.*;
import software.amazon.awssdk.services.s3.presigner.S3Presigner;
import software.amazon.awssdk.transfer.s3.S3TransferManager;
import software.amazon.awssdk.transfer.s3.model.DownloadRequest;
import software.amazon.awssdk.transfer.s3.model.UploadRequest;

import java.net.URI;
import java.time.Duration;
import java.util.*;

/** aws-sdk-java-v2: sync S3Client (default HTTP client) or CRT-based S3AsyncClient. */
final class V2Suite extends Suite {
    private final boolean crt;
    private final URI endpoint;
    private final StaticCredentialsProvider creds;
    private final Region region;
    private final V2Ops s3;
    private final S3AsyncClient asyncClient; // used by S3TransferManager
    private final S3Presigner presigner;
    private final List<String> extraBuckets = Collections.synchronizedList(new ArrayList<>());

    V2Suite(Env env, String sdk, boolean crt) {
        super(env, sdk);
        this.crt = crt;
        this.endpoint = URI.create(env.endpoint());
        this.creds = StaticCredentialsProvider.create(AwsBasicCredentials.create(env.accessKey(), env.secretKey()));
        this.region = Region.of(env.region());
        if (crt) {
            asyncClient = crtClient(creds);
            s3 = V2Ops.async(asyncClient);
        } else {
            s3 = V2Ops.sync(syncClient(creds));
            // Java-based (non-CRT) async client with multipart support, used only by S3TransferManager.
            asyncClient = S3AsyncClient.builder().endpointOverride(endpoint).region(region).credentialsProvider(creds)
                    .forcePathStyle(true).multipartEnabled(true).build();
        }
        presigner = S3Presigner.builder().endpointOverride(endpoint).region(region).credentialsProvider(creds)
                .serviceConfiguration(S3Configuration.builder().pathStyleAccessEnabled(true).build()).build();
    }

    private S3Client syncClient(StaticCredentialsProvider c) {
        return S3Client.builder().endpointOverride(endpoint).region(region).credentialsProvider(c).forcePathStyle(true).build();
    }

    private S3AsyncClient crtClient(StaticCredentialsProvider c) {
        return S3AsyncClient.crtBuilder().endpointOverride(endpoint).region(region).credentialsProvider(c)
                .forcePathStyle(true)
                .thresholdInBytes(16L * U.MIB) // keep put_get_large (8 MiB) a single PUT
                .minimumPartSizeInBytes(8L * U.MIB)
                .build();
    }

    @Override
    int status(Throwable t) {
        return t instanceof SdkServiceException e ? e.statusCode() : -1;
    }

    @Override
    String code(Throwable t) {
        if (t instanceof AwsServiceException e && e.awsErrorDetails() != null) return e.awsErrorDetails().errorCode();
        return null;
    }

    // ---- helpers ----

    private PutObjectResponse put(String key, byte[] data) {
        return s3.putObject(PutObjectRequest.builder().bucket(bucket).key(key).build(), data);
    }

    private ResponseBytes<GetObjectResponse> get(String key) {
        return s3.getObject(GetObjectRequest.builder().bucket(bucket).key(key).build());
    }

    private ResponseBytes<GetObjectResponse> getRange(String key, String range) {
        return s3.getObject(GetObjectRequest.builder().bucket(bucket).key(key).range(range).build());
    }

    private List<String> listKeys(String b, String prefix) {
        List<String> keys = new ArrayList<>();
        String token = null;
        do {
            ListObjectsV2Response r = s3.listObjectsV2(ListObjectsV2Request.builder().bucket(b).prefix(prefix).continuationToken(token).build());
            r.contents().forEach(o -> keys.add(o.key()));
            token = Boolean.TRUE.equals(r.isTruncated()) ? r.nextContinuationToken() : null;
        } while (token != null);
        return keys;
    }

    private void emptyBucket(String b) {
        List<String> keys = listKeys(b, null);
        for (int i = 0; i < keys.size(); i += 1000) {
            List<ObjectIdentifier> ids = keys.subList(i, Math.min(keys.size(), i + 1000)).stream()
                    .map(k -> ObjectIdentifier.builder().key(k).build()).toList();
            s3.deleteObjects(DeleteObjectsRequest.builder().bucket(b).delete(Delete.builder().objects(ids).build()).build());
        }
    }

    private void destroyBucket(String b) {
        try {
            ListObjectVersionsResponse r;
            do {
                r = s3.listObjectVersions(ListObjectVersionsRequest.builder().bucket(b).build());
                for (ObjectVersion v : r.versions())
                    s3.deleteObject(DeleteObjectRequest.builder().bucket(b).key(v.key()).versionId(v.versionId()).build());
                for (DeleteMarkerEntry m : r.deleteMarkers())
                    s3.deleteObject(DeleteObjectRequest.builder().bucket(b).key(m.key()).versionId(m.versionId()).build());
            } while (Boolean.TRUE.equals(r.isTruncated()) && (!r.versions().isEmpty() || !r.deleteMarkers().isEmpty()));
        } catch (Exception ignored) {
        }
        try { emptyBucket(b); } catch (Exception ignored) {}
        try { s3.deleteBucket(DeleteBucketRequest.builder().bucket(b).build()); } catch (Exception ignored) {}
    }

    private String newBucket(String suffix) {
        String b = bucket + "-" + suffix;
        extraBuckets.add(b);
        s3.createBucket(CreateBucketRequest.builder().bucket(b).build());
        return b;
    }

    @Override
    void cleanup() {
        for (String b : extraBuckets) destroyBucket(b);
        destroyBucket(bucket);
        presigner.close();
        s3.close();
        if (!crt) asyncClient.close();
    }

    // ---- scenarios ----

    @Override
    Map<String, Body> scenarios() {
        Map<String, Body> m = new LinkedHashMap<>();
        m.put("create_bucket", () -> s3.createBucket(CreateBucketRequest.builder().bucket(bucket).build()));
        m.put("head_bucket", () -> s3.headBucket(HeadBucketRequest.builder().bucket(bucket).build()));
        m.put("head_bucket_missing", () -> expectError("HeadBucket missing",
                () -> s3.headBucket(HeadBucketRequest.builder().bucket("m-missing-" + randLower(12)).build()),
                Set.of(404), "NoSuchBucket", "NotFound"));
        m.put("list_buckets", () -> {
            List<String> names = s3.listBuckets(ListBucketsRequest.builder().build()).buckets().stream().map(Bucket::name).toList();
            U.check(names.contains(bucket), "bucket " + bucket + " not in ListBuckets (" + names.size() + " buckets)");
        });
        m.put("put_get_small", () -> {
            byte[] data = U.utf8("hello world");
            PutObjectResponse p = put("small.txt", data);
            ResponseBytes<GetObjectResponse> g = get("small.txt");
            U.bytesEq(data, g.asByteArray(), "GET body");
            U.eq(U.md5hex(data), U.unquote(p.eTag()), "PUT ETag");
            U.eq(U.md5hex(data), U.unquote(g.response().eTag()), "GET ETag");
        });
        m.put("put_get_empty", () -> {
            put("empty", new byte[0]);
            ResponseBytes<GetObjectResponse> g = get("empty");
            U.eq(0, g.asByteArray().length, "body length");
            U.eq(0L, g.response().contentLength(), "Content-Length");
        });
        m.put("put_get_large", () -> {
            byte[] data = U.rand(8 * U.MIB);
            put("large.bin", data);
            U.bytesEq(data, get("large.bin").asByteArray(), "GET body");
        });
        m.put("special_keys", () -> {
            List<String> errs = new ArrayList<>();
            for (String k : U.SPECIAL_KEYS) {
                try {
                    byte[] data = U.utf8("key=" + k);
                    put(k, data);
                    U.bytesEq(data, get(k).asByteArray(), "GET");
                    List<String> keys = listKeys(bucket, k);
                    U.check(keys.contains(k), "not in List: " + keys);
                } catch (Exception | AssertionError e) {
                    errs.add("[" + k + "] " + U.describe(U.unwrap(e)));
                }
            }
            U.check(errs.isEmpty(), String.join("; ", errs));
        });
        m.put("head_object_metadata", () -> {
            s3.putObject(PutObjectRequest.builder().bucket(bucket).key("meta.txt").contentType("text/plain; charset=utf-8")
                    .cacheControl("max-age=3600").contentDisposition("attachment; filename=\"m.txt\"").contentEncoding("gzip")
                    .metadata(U.USER_META).build(), U.utf8("metadata"));
            HeadObjectResponse h = s3.headObject(HeadObjectRequest.builder().bucket(bucket).key("meta.txt").build());
            U.eq("text/plain; charset=utf-8", h.contentType(), "Content-Type");
            U.eq("max-age=3600", h.cacheControl(), "Cache-Control");
            U.eq("attachment; filename=\"m.txt\"", h.contentDisposition(), "Content-Disposition");
            U.eq("gzip", h.contentEncoding(), "Content-Encoding");
            U.eq(U.USER_META, h.metadata(), "user metadata");
        });
        m.put("get_range", () -> {
            byte[] data = U.rand(100);
            put("range.bin", data);
            ResponseBytes<GetObjectResponse> g = getRange("range.bin", "bytes=10-19");
            U.bytesEq(Arrays.copyOfRange(data, 10, 20), g.asByteArray(), "bytes=10-19");
            U.eq("bytes 10-19/100", g.response().contentRange(), "Content-Range 10-19");
            g = getRange("range.bin", "bytes=-5");
            U.bytesEq(Arrays.copyOfRange(data, 95, 100), g.asByteArray(), "bytes=-5");
            U.eq("bytes 95-99/100", g.response().contentRange(), "Content-Range -5");
            g = getRange("range.bin", "bytes=95-");
            U.bytesEq(Arrays.copyOfRange(data, 95, 100), g.asByteArray(), "bytes=95-");
            U.eq("bytes 95-99/100", g.response().contentRange(), "Content-Range 95-");
        });
        m.put("get_range_invalid", () -> {
            put("range-inv.bin", U.rand(100));
            expectError("GET bytes=200-300", () -> getRange("range-inv.bin", "bytes=200-300"), Set.of(416), "InvalidRange");
        });
        m.put("get_missing_key", () -> expectError("GET missing", () -> get("no-such-key-" + randLower(8)), Set.of(404), "NoSuchKey"));
        m.put("get_if_none_match", () -> {
            String etag = put("inm.txt", U.utf8("if-none-match")).eTag();
            expectError("GET If-None-Match", () -> s3.getObject(GetObjectRequest.builder().bucket(bucket).key("inm.txt").ifNoneMatch(etag).build()),
                    Set.of(304), "NotModified", "304");
        });
        m.put("get_if_match_fail", () -> {
            put("im.txt", U.utf8("if-match"));
            expectError("GET If-Match wrong", () -> s3.getObject(GetObjectRequest.builder().bucket(bucket).key("im.txt").ifMatch("\"wrong\"").build()),
                    Set.of(412), "PreconditionFailed");
        });
        m.put("put_if_none_match", () -> {
            put("pinm.txt", U.utf8("first"));
            expectError("PUT If-None-Match:* existing", () -> s3.putObject(PutObjectRequest.builder().bucket(bucket).key("pinm.txt").ifNoneMatch("*").build(), U.utf8("second")),
                    Set.of(412), "PreconditionFailed");
            String fresh = "pinm-new-" + randLower(8);
            s3.putObject(PutObjectRequest.builder().bucket(bucket).key(fresh).ifNoneMatch("*").build(), U.utf8("new"));
            U.bytesEq(U.utf8("new"), get(fresh).asByteArray(), "new key body");
            U.bytesEq(U.utf8("first"), get("pinm.txt").asByteArray(), "existing key unchanged");
        });
        m.put("delete_object", () -> {
            put("del.txt", U.utf8("x"));
            s3.deleteObject(DeleteObjectRequest.builder().bucket(bucket).key("del.txt").build());
            expectError("HEAD deleted", () -> s3.headObject(HeadObjectRequest.builder().bucket(bucket).key("del.txt").build()), Set.of(404), "NoSuchKey", "NotFound");
        });
        m.put("delete_missing_object", () -> s3.deleteObject(DeleteObjectRequest.builder().bucket(bucket).key("never-existed-" + randLower(8)).build()));
        m.put("delete_objects", () -> {
            List<String> keys = List.of("dm/1", "dm/2", "dm/3");
            for (String k : keys) put(k, U.utf8(k));
            DeleteObjectsResponse r = s3.deleteObjects(DeleteObjectsRequest.builder().bucket(bucket).delete(Delete.builder()
                    .objects(keys.stream().map(k -> ObjectIdentifier.builder().key(k).build()).toList()).build()).build());
            U.check(r.errors().isEmpty(), "DeleteObjects errors: " + r.errors());
            U.eq(List.of(), listKeys(bucket, "dm/"), "keys left after DeleteObjects");
        });
        m.put("list_v2_basic", () -> {
            Map<String, byte[]> objs = new TreeMap<>(Map.of("lb/c", U.rand(3), "lb/a", U.rand(1), "lb/b", U.rand(2)));
            for (var e : objs.entrySet()) put(e.getKey(), e.getValue());
            ListObjectsV2Response r = s3.listObjectsV2(ListObjectsV2Request.builder().bucket(bucket).prefix("lb/").build());
            U.eq(new ArrayList<>(objs.keySet()), r.contents().stream().map(S3Object::key).toList(), "keys");
            for (S3Object o : r.contents()) {
                U.eq((long) objs.get(o.key()).length, o.size(), "Size of " + o.key());
                U.eq(U.md5hex(objs.get(o.key())), U.unquote(o.eTag()), "ETag of " + o.key());
            }
        });
        m.put("list_v2_prefix_delimiter", () -> {
            String b = newBucket("pd");
            for (String k : List.of("a/1", "a/2", "b/1", "c"))
                s3.putObject(PutObjectRequest.builder().bucket(b).key(k).build(), U.utf8(k));
            ListObjectsV2Response r = s3.listObjectsV2(ListObjectsV2Request.builder().bucket(b).prefix("").delimiter("/").build());
            U.eq(List.of("a/", "b/"), r.commonPrefixes().stream().map(CommonPrefix::prefix).toList(), "CommonPrefixes");
            U.eq(List.of("c"), r.contents().stream().map(S3Object::key).toList(), "Contents");
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
                ListObjectsV2Response r = s3.listObjectsV2(ListObjectsV2Request.builder().bucket(bucket).prefix("pg/").maxKeys(10).continuationToken(token).build());
                pages++;
                U.check(r.contents().size() <= 10, "page " + pages + " has " + r.contents().size() + " keys");
                r.contents().forEach(o -> got.add(o.key()));
                token = Boolean.TRUE.equals(r.isTruncated()) ? r.nextContinuationToken() : null;
                U.check(pages <= 10, "too many pages");
            } while (token != null);
            U.eq(3, pages, "page count");
            U.eq(expected, got, "keys across pages");
        });
        m.put("list_v2_start_after", () -> {
            for (int i = 0; i < 5; i++) put("sa/k" + i, U.utf8("x"));
            ListObjectsV2Response r = s3.listObjectsV2(ListObjectsV2Request.builder().bucket(bucket).prefix("sa/").startAfter("sa/k2").build());
            U.eq(List.of("sa/k3", "sa/k4"), r.contents().stream().map(S3Object::key).toList(), "keys after sa/k2");
        });
        m.put("list_v1", () -> {
            for (int i = 0; i < 5; i++) put("l1/k" + i, U.utf8("x"));
            ListObjectsResponse r = s3.listObjects(ListObjectsRequest.builder().bucket(bucket).prefix("l1/").marker("l1/k1").build());
            U.eq(List.of("l1/k2", "l1/k3", "l1/k4"), r.contents().stream().map(S3Object::key).toList(), "keys after marker l1/k1");
            ListObjectsResponse page = s3.listObjects(ListObjectsRequest.builder().bucket(bucket).prefix("l1/").maxKeys(2).build());
            U.eq(List.of("l1/k0", "l1/k1"), page.contents().stream().map(S3Object::key).toList(), "first page MaxKeys=2");
            U.eq(true, page.isTruncated(), "IsTruncated");
        });
        m.put("copy_object", () -> {
            byte[] data = U.utf8("copy me");
            s3.putObject(PutObjectRequest.builder().bucket(bucket).key("cp/src").contentType("text/x-copy").metadata(Map.of("color", "Blue")).build(), data);
            s3.copyObject(CopyObjectRequest.builder().sourceBucket(bucket).sourceKey("cp/src").destinationBucket(bucket).destinationKey("cp/dst").build());
            ResponseBytes<GetObjectResponse> g = get("cp/dst");
            U.bytesEq(data, g.asByteArray(), "copied body");
            U.eq(Map.of("color", "Blue"), g.response().metadata(), "copied metadata");
            U.eq("text/x-copy", g.response().contentType(), "copied Content-Type");
        });
        m.put("copy_object_replace_metadata", () -> {
            byte[] data = U.utf8("copy me");
            s3.putObject(PutObjectRequest.builder().bucket(bucket).key("cpr/src").metadata(Map.of("color", "Blue", "old", "1")).build(), data);
            s3.copyObject(CopyObjectRequest.builder().sourceBucket(bucket).sourceKey("cpr/src").destinationBucket(bucket).destinationKey("cpr/dst")
                    .metadataDirective(MetadataDirective.REPLACE).metadata(Map.of("color", "Red", "new", "2")).contentType("text/x-replaced").build());
            HeadObjectResponse h = s3.headObject(HeadObjectRequest.builder().bucket(bucket).key("cpr/dst").build());
            U.eq(Map.of("color", "Red", "new", "2"), h.metadata(), "replaced metadata");
            U.eq("text/x-replaced", h.contentType(), "replaced Content-Type");
            U.bytesEq(data, get("cpr/dst").asByteArray(), "copied body");
        });
        m.put("multipart_upload", () -> {
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(5 * U.MIB), U.rand(1024)};
            String key = "mpu/obj";
            String id = s3.createMultipartUpload(CreateMultipartUploadRequest.builder().bucket(bucket).key(key).build()).uploadId();
            List<CompletedPart> done = new ArrayList<>();
            for (int i = 0; i < parts.length; i++) {
                UploadPartResponse r = s3.uploadPart(UploadPartRequest.builder().bucket(bucket).key(key).uploadId(id).partNumber(i + 1).build(), parts[i]);
                done.add(CompletedPart.builder().partNumber(i + 1).eTag(r.eTag()).build());
            }
            CompleteMultipartUploadResponse c = s3.completeMultipartUpload(CompleteMultipartUploadRequest.builder().bucket(bucket).key(key).uploadId(id)
                    .multipartUpload(CompletedMultipartUpload.builder().parts(done).build()).build());
            String etag = U.unquote(c.eTag());
            U.check(etag != null && etag.matches("[0-9a-f]{32}-3"), "multipart ETag not <hex>-3: " + c.eTag());
            byte[] all = concat(parts);
            ResponseBytes<GetObjectResponse> g = get(key);
            U.bytesEq(all, g.asByteArray(), "GET body");
        });
        m.put("multipart_abort", () -> {
            String key = "mpu/abort";
            String id = s3.createMultipartUpload(CreateMultipartUploadRequest.builder().bucket(bucket).key(key).build()).uploadId();
            s3.uploadPart(UploadPartRequest.builder().bucket(bucket).key(key).uploadId(id).partNumber(1).build(), U.rand(1024));
            s3.abortMultipartUpload(AbortMultipartUploadRequest.builder().bucket(bucket).key(key).uploadId(id).build());
            expectError("ListParts after abort", () -> s3.listParts(ListPartsRequest.builder().bucket(bucket).key(key).uploadId(id).build()),
                    Set.of(404), "NoSuchUpload");
        });
        m.put("multipart_list_parts", () -> {
            String key = "mpu/list";
            byte[][] parts = {U.rand(5 * U.MIB), U.rand(1024)};
            String id = s3.createMultipartUpload(CreateMultipartUploadRequest.builder().bucket(bucket).key(key).build()).uploadId();
            try {
                for (int i = 0; i < parts.length; i++)
                    s3.uploadPart(UploadPartRequest.builder().bucket(bucket).key(key).uploadId(id).partNumber(i + 1).build(), parts[i]);
                ListPartsResponse r = s3.listParts(ListPartsRequest.builder().bucket(bucket).key(key).uploadId(id).build());
                U.eq(2, r.parts().size(), "part count");
                for (int i = 0; i < 2; i++) {
                    Part p = r.parts().get(i);
                    U.eq(i + 1, p.partNumber(), "PartNumber");
                    U.eq((long) parts[i].length, p.size(), "Size of part " + (i + 1));
                    U.eq(U.md5hex(parts[i]), U.unquote(p.eTag()), "ETag of part " + (i + 1));
                }
            } finally {
                try { s3.abortMultipartUpload(AbortMultipartUploadRequest.builder().bucket(bucket).key(key).uploadId(id).build()); } catch (Exception ignored) {}
            }
        });
        m.put("multipart_too_small", () -> {
            String key = "mpu/small";
            String id = s3.createMultipartUpload(CreateMultipartUploadRequest.builder().bucket(bucket).key(key).build()).uploadId();
            try {
                List<CompletedPart> done = new ArrayList<>();
                for (int i = 1; i <= 2; i++) {
                    UploadPartResponse r = s3.uploadPart(UploadPartRequest.builder().bucket(bucket).key(key).uploadId(id).partNumber(i).build(), U.rand(1024));
                    done.add(CompletedPart.builder().partNumber(i).eTag(r.eTag()).build());
                }
                expectError("Complete with 1 KiB parts", () -> s3.completeMultipartUpload(CompleteMultipartUploadRequest.builder().bucket(bucket).key(key).uploadId(id)
                        .multipartUpload(CompletedMultipartUpload.builder().parts(done).build()).build()), Set.of(), "EntityTooSmall");
            } finally {
                try { s3.abortMultipartUpload(AbortMultipartUploadRequest.builder().bucket(bucket).key(key).uploadId(id).build()); } catch (Exception ignored) {}
            }
        });
        m.put("high_level_upload", () -> {
            byte[] data = U.rand(20 * U.MIB);
            try (S3TransferManager tm = S3TransferManager.builder().s3Client(asyncClient).build()) {
                tm.upload(UploadRequest.builder().putObjectRequest(b -> b.bucket(bucket).key("tm/obj")).requestBody(AsyncRequestBody.fromBytes(data)).build())
                        .completionFuture().join();
                ResponseBytes<GetObjectResponse> got = tm.download(DownloadRequest.builder().getObjectRequest(b -> b.bucket(bucket).key("tm/obj"))
                        .responseTransformer(AsyncResponseTransformer.toBytes()).build()).completionFuture().join().result();
                U.bytesEq(data, got.asByteArray(), "downloaded body");
            }
        });
        m.put("presigned_get", () -> {
            byte[] data = U.utf8("presigned get body");
            put("ps/get", data);
            String url = presigner.presignGetObject(r -> r.signatureDuration(Duration.ofMinutes(10))
                    .getObjectRequest(g -> g.bucket(bucket).key("ps/get"))).url().toString();
            U.bytesEq(data, U.httpGet(url), "presigned GET body");
        });
        m.put("presigned_put", () -> {
            byte[] data = U.utf8("presigned put body");
            String url = presigner.presignPutObject(r -> r.signatureDuration(Duration.ofMinutes(10))
                    .putObjectRequest(p -> p.bucket(bucket).key("ps/put"))).url().toString();
            U.httpPut(url, data);
            U.bytesEq(data, get("ps/put").asByteArray(), "SDK GET after presigned PUT");
        });
        for (String alg : U.CHECKSUM_ALGS) {
            m.put("checksum_" + alg.toLowerCase(Locale.ROOT), () -> {
                byte[] data = U.rand(64 * 1024 + 7);
                String key = "ck/" + alg;
                String expected = U.checksum(alg, data);
                s3.putObject(PutObjectRequest.builder().bucket(bucket).key(key).checksumAlgorithm(ChecksumAlgorithm.fromValue(alg)).build(), data);
                ResponseBytes<GetObjectResponse> g = s3.getObject(GetObjectRequest.builder().bucket(bucket).key(key).checksumMode(ChecksumMode.ENABLED).build());
                U.bytesEq(data, g.asByteArray(), "GET body");
                GetObjectResponse r = g.response();
                String got = switch (alg) {
                    case "CRC32" -> r.checksumCRC32();
                    case "CRC32C" -> r.checksumCRC32C();
                    case "SHA1" -> r.checksumSHA1();
                    case "SHA256" -> r.checksumSHA256();
                    default -> r.checksumCRC64NVME();
                };
                U.eq(expected, got, "x-amz-checksum-" + alg.toLowerCase(Locale.ROOT));
            });
        }
        m.put("content_md5_bad", () -> expectError("PUT bad Content-MD5",
                () -> s3.putObject(PutObjectRequest.builder().bucket(bucket).key("badmd5").contentMD5(U.md5b64(U.utf8("something else"))).build(), U.utf8("payload")),
                Set.of(400), "BadDigest", "InvalidDigest"));
        m.put("bad_credentials", () -> {
            StaticCredentialsProvider bad = StaticCredentialsProvider.create(AwsBasicCredentials.create(env.accessKey(), "wrong-secret-key"));
            V2Ops c = crt ? V2Ops.async(crtClient(bad)) : V2Ops.sync(syncClient(bad));
            try (c) {
                expectError("ListBuckets with bad secret", () -> c.listBuckets(ListBucketsRequest.builder().build()),
                        Set.of(403), "SignatureDoesNotMatch", "InvalidAccessKeyId");
            }
        });
        m.put("delete_bucket_not_empty", () -> {
            put("nonempty/x", U.utf8("x"));
            expectError("DeleteBucket non-empty", () -> s3.deleteBucket(DeleteBucketRequest.builder().bucket(bucket).build()), Set.of(409), "BucketNotEmpty");
        });
        m.put("versioning", () -> {
            String b = newBucket("ver");
            s3.putBucketVersioning(PutBucketVersioningRequest.builder().bucket(b)
                    .versioningConfiguration(VersioningConfiguration.builder().status(BucketVersioningStatus.ENABLED).build()).build());
            String v1 = s3.putObject(PutObjectRequest.builder().bucket(b).key("doc").build(), U.utf8("version-1")).versionId();
            String v2 = s3.putObject(PutObjectRequest.builder().bucket(b).key("doc").build(), U.utf8("version-2")).versionId();
            U.check(v1 != null && v2 != null && !v1.equals(v2), "PUT returned version ids " + v1 + ", " + v2);
            List<ObjectVersion> vs = s3.listObjectVersions(ListObjectVersionsRequest.builder().bucket(b).prefix("doc").build()).versions();
            U.eq(2, vs.size(), "version count");
            U.eq(Set.of(v1, v2), new HashSet<>(vs.stream().map(ObjectVersion::versionId).toList()), "listed version ids");
            byte[] old = s3.getObject(GetObjectRequest.builder().bucket(b).key("doc").versionId(v1).build()).asByteArray();
            U.bytesEq(U.utf8("version-1"), old, "GET old version");
            U.bytesEq(U.utf8("version-2"), s3.getObject(GetObjectRequest.builder().bucket(b).key("doc").build()).asByteArray(), "GET latest");
        });
        m.put("object_tagging", () -> {
            put("tag.txt", U.utf8("tagged"));
            Map<String, String> tags = Map.of("project", "matrix", "env", "test");
            s3.putObjectTagging(PutObjectTaggingRequest.builder().bucket(bucket).key("tag.txt").tagging(Tagging.builder()
                    .tagSet(tags.entrySet().stream().map(e -> Tag.builder().key(e.getKey()).value(e.getValue()).build()).toList()).build()).build());
            Map<String, String> got = new HashMap<>();
            s3.getObjectTagging(GetObjectTaggingRequest.builder().bucket(bucket).key("tag.txt").build()).tagSet().forEach(t -> got.put(t.key(), t.value()));
            U.eq(tags, got, "tags");
        });
        m.put("delete_bucket", () -> {
            emptyBucket(bucket);
            s3.deleteBucket(DeleteBucketRequest.builder().bucket(bucket).build());
            expectError("HeadBucket after delete", () -> s3.headBucket(HeadBucketRequest.builder().bucket(bucket).build()), Set.of(404), "NoSuchBucket", "NotFound");
        });
        return m;
    }

    static byte[] concat(byte[][] parts) {
        int n = 0;
        for (byte[] p : parts) n += p.length;
        byte[] all = new byte[n];
        int off = 0;
        for (byte[] p : parts) { System.arraycopy(p, 0, all, off, p.length); off += p.length; }
        return all;
    }
}
