// aws-sdk v2 (legacy, maintenance mode)
process.env.AWS_SDK_JS_SUPPRESS_MAINTENANCE_MODE_MESSAGE = "1";
const { default: AWS } = await import("aws-sdk");
import {
  cfg, runSuite, uniqueBucket, pkgVersion, rand, md5hex, unquote, check, eq, bufEq, deepEq, expectError,
  checksumB64, skip, SPECIAL_KEYS, META, KiB, MiB,
} from "./common.mjs";

const SDK = "aws-sdk-js-v2";
const mk = (secret = cfg.secretKey) => new AWS.S3({
  endpoint: cfg.endpoint, region: cfg.region, s3ForcePathStyle: true, signatureVersion: "v4",
  accessKeyId: cfg.accessKey, secretAccessKey: secret, maxRetries: 0,
});
const s3 = mk();
const call = (op, params, client = s3) => client[op](params).promise();
const B = uniqueBucket("awsv2");
const VB = `${B}-v`;
const DB = `${B}-d`; // dedicated bucket for delete_bucket

const put = (Key, Body, extra = {}) => call("putObject", { Bucket: B, Key, Body, ...extra });
async function get(Key, extra = {}, Bucket = B) {
  const r = await call("getObject", { Bucket, Key, ...extra });
  return { r, body: Buffer.from(r.Body ?? []) };
}
const NOT_FOUND = { codes: ["NotFound", "NoSuchKey", "NoSuchBucket"], statuses: [404] };

async function listAll(Prefix) {
  const keys = [];
  let ContinuationToken;
  do {
    const r = await call("listObjectsV2", { Bucket: B, Prefix, ContinuationToken });
    keys.push(...(r.Contents ?? []).map((o) => o.Key));
    ContinuationToken = r.IsTruncated ? r.NextContinuationToken : undefined;
  } while (ContinuationToken);
  return keys;
}

// v2 sends the x-amz-checksum-* header it is given but does not compute it, so
// the harness supplies the value alongside ChecksumAlgorithm.
function checksumScenario(alg) {
  return async () => {
    const data = rand(1000), key = `ck/${alg}`;
    await put(key, data, { ChecksumAlgorithm: alg, [`Checksum${alg}`]: checksumB64(alg, data) });
    const { r, body } = await get(key, { ChecksumMode: "ENABLED" });
    bufEq(body, data);
    eq(r[`Checksum${alg}`], checksumB64(alg, data), `Checksum${alg}`);
  };
}

const impl = {
  create_bucket: () => call("createBucket", { Bucket: B }),
  head_bucket: () => call("headBucket", { Bucket: B }),
  head_bucket_missing: () => expectError(() => call("headBucket", { Bucket: `${B}-missing` }), NOT_FOUND),
  list_buckets: async () => {
    const r = await call("listBuckets", {});
    check((r.Buckets ?? []).some((b) => b.Name === B), `bucket ${B} not in ListBuckets`);
  },
  put_get_small: async () => {
    const data = Buffer.from("hello world");
    const p = await put("small.txt", data);
    const { r, body } = await get("small.txt");
    bufEq(body, data);
    eq(unquote(p.ETag), md5hex(data), "PUT ETag");
    eq(unquote(r.ETag), md5hex(data), "GET ETag");
  },
  put_get_empty: async () => {
    await put("empty", Buffer.alloc(0));
    const { r, body } = await get("empty");
    eq(body.length, 0, "body length");
    eq(Number(r.ContentLength), 0, "ContentLength");
  },
  put_get_large: async () => {
    const data = rand(8 * MiB);
    await put("large.bin", data);
    bufEq((await get("large.bin")).body, data);
  },
  special_keys: async () => {
    for (const k of SPECIAL_KEYS) {
      const data = Buffer.from(`data:${k}`);
      await put(k, data);
      bufEq((await get(k)).body, data, `body of ${k}`);
    }
    const keys = await listAll();
    for (const k of SPECIAL_KEYS) check(keys.includes(k), `key ${JSON.stringify(k)} missing from list`);
  },
  head_object_metadata: async () => {
    await put("meta.txt", Buffer.from("meta"), {
      ContentType: "text/plain", CacheControl: "max-age=60", ContentDisposition: 'attachment; filename="m.txt"',
      ContentEncoding: "gzip", Metadata: META,
    });
    const r = await call("headObject", { Bucket: B, Key: "meta.txt" });
    eq(r.ContentType, "text/plain", "ContentType");
    eq(r.CacheControl, "max-age=60", "CacheControl");
    eq(r.ContentDisposition, 'attachment; filename="m.txt"', "ContentDisposition");
    eq(r.ContentEncoding, "gzip", "ContentEncoding");
    deepEq(r.Metadata, { color: "Blue", "x-y": "z" }, "Metadata");
  },
  get_range: async () => {
    const data = rand(100);
    await put("range.bin", data);
    for (const [range, start, end] of [["bytes=10-19", 10, 19], ["bytes=-5", 95, 99], ["bytes=95-", 95, 99]]) {
      const { r, body } = await get("range.bin", { Range: range });
      bufEq(body, data.subarray(start, end + 1), `body for ${range}`);
      eq(r.ContentRange, `bytes ${start}-${end}/100`, `ContentRange for ${range}`);
    }
  },
  get_range_invalid: async () => {
    await put("range-inv.bin", rand(100));
    await expectError(() => get("range-inv.bin", { Range: "bytes=200-300" }), { codes: ["InvalidRange"], statuses: [416] });
  },
  get_missing_key: () => expectError(() => get("does/not/exist"), { codes: ["NoSuchKey"], statuses: [404] }),
  get_if_none_match: async () => {
    const p = await put("cond.txt", Buffer.from("cond"));
    await expectError(() => get("cond.txt", { IfNoneMatch: p.ETag }), { codes: ["NotModified"], statuses: [304] });
  },
  get_if_match_fail: async () => {
    await put("cond2.txt", Buffer.from("cond"));
    await expectError(() => get("cond2.txt", { IfMatch: '"wrong"' }), { codes: ["PreconditionFailed"], statuses: [412] });
  },
  put_if_none_match: async () => {
    const key = `inm-${Date.now()}`;
    await put(key, Buffer.from("first"), { IfNoneMatch: "*" });
    await expectError(() => put(key, Buffer.from("second"), { IfNoneMatch: "*" }), { codes: ["PreconditionFailed"], statuses: [412] });
    bufEq((await get(key)).body, Buffer.from("first"));
  },
  delete_object: async () => {
    await put("del.txt", Buffer.from("x"));
    await call("deleteObject", { Bucket: B, Key: "del.txt" });
    await expectError(() => call("headObject", { Bucket: B, Key: "del.txt" }), NOT_FOUND);
  },
  delete_missing_object: async () => {
    const req = s3.deleteObject({ Bucket: B, Key: "never-existed" });
    await req.promise();
    eq(req.response.httpResponse.statusCode, 204, "status");
  },
  delete_objects: async () => {
    const keys = ["dm/1", "dm/2", "dm/3"];
    for (const k of keys) await put(k, Buffer.from(k));
    const r = await call("deleteObjects", { Bucket: B, Delete: { Objects: keys.map((Key) => ({ Key })) } });
    eq((r.Errors ?? []).length, 0, "Errors");
    deepEq((r.Deleted ?? []).map((d) => d.Key).sort(), keys, "Deleted");
    deepEq(await listAll("dm/"), [], "remaining keys");
  },
  list_v2_basic: async () => {
    const objs = { "lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7) };
    for (const [k, v] of Object.entries(objs)) await put(k, v);
    const r = await call("listObjectsV2", { Bucket: B, Prefix: "lb/" });
    deepEq(r.Contents.map((o) => o.Key), ["lb/a", "lb/b", "lb/c"], "keys");
    for (const o of r.Contents) {
      eq(o.Size, objs[o.Key].length, `Size of ${o.Key}`);
      eq(unquote(o.ETag), md5hex(objs[o.Key]), `ETag of ${o.Key}`);
    }
    eq(r.KeyCount, 3, "KeyCount");
  },
  list_v2_prefix_delimiter: async () => {
    for (const k of ["a/1", "a/2", "b/1", "c"]) await put(`pd/${k}`, Buffer.from(k));
    const r = await call("listObjectsV2", { Bucket: B, Prefix: "pd/", Delimiter: "/" });
    deepEq((r.CommonPrefixes ?? []).map((p) => p.Prefix), ["pd/a/", "pd/b/"], "CommonPrefixes");
    deepEq((r.Contents ?? []).map((o) => o.Key), ["pd/c"], "Contents");
  },
  list_v2_pagination: async () => {
    const keys = Array.from({ length: 25 }, (_, i) => `pg/${String(i).padStart(2, "0")}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const pages = [];
    let ContinuationToken;
    do {
      const r = await call("listObjectsV2", { Bucket: B, Prefix: "pg/", MaxKeys: 10, ContinuationToken });
      pages.push((r.Contents ?? []).map((o) => o.Key));
      ContinuationToken = r.IsTruncated ? r.NextContinuationToken : undefined;
      check(pages.length <= 5, "too many pages");
    } while (ContinuationToken);
    deepEq(pages.map((p) => p.length), [10, 10, 5], "page sizes");
    deepEq(pages.flat(), keys, "keys");
  },
  list_v2_start_after: async () => {
    const keys = Array.from({ length: 10 }, (_, i) => `sa/${i}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const r = await call("listObjectsV2", { Bucket: B, Prefix: "sa/", StartAfter: "sa/4" });
    deepEq((r.Contents ?? []).map((o) => o.Key), keys.slice(5), "keys");
  },
  list_v1: async () => {
    const keys = Array.from({ length: 6 }, (_, i) => `l1/${i}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const r1 = await call("listObjects", { Bucket: B, Prefix: "l1/", Marker: "l1/2", MaxKeys: 2 });
    deepEq((r1.Contents ?? []).map((o) => o.Key), ["l1/3", "l1/4"], "page 1");
    eq(r1.IsTruncated, true, "IsTruncated");
    const r2 = await call("listObjects", { Bucket: B, Prefix: "l1/", Marker: "l1/4" });
    deepEq((r2.Contents ?? []).map((o) => o.Key), ["l1/5"], "page 2");
    eq(!!r2.IsTruncated, false, "IsTruncated page 2");
  },
  copy_object: async () => {
    const data = rand(1234);
    await put("cp/src", data, { ContentType: "application/x-test", Metadata: META });
    await call("copyObject", { Bucket: B, Key: "cp/dst", CopySource: `${B}/cp/src` });
    const { r, body } = await get("cp/dst");
    bufEq(body, data);
    eq(r.ContentType, "application/x-test", "ContentType");
    deepEq(r.Metadata, { color: "Blue", "x-y": "z" }, "Metadata");
  },
  copy_object_replace_metadata: async () => {
    const data = rand(100);
    await put("cpr/src", data, { ContentType: "application/x-old", Metadata: META });
    await call("copyObject", {
      Bucket: B, Key: "cpr/dst", CopySource: `${B}/cpr/src`, MetadataDirective: "REPLACE",
      ContentType: "application/x-new", Metadata: { fresh: "yes" },
    });
    const r = await call("headObject", { Bucket: B, Key: "cpr/dst" });
    eq(r.ContentType, "application/x-new", "ContentType");
    deepEq(r.Metadata, { fresh: "yes" }, "Metadata");
    bufEq((await get("cpr/dst")).body, data);
  },
  multipart_upload: async () => {
    const key = "mpu/obj", parts = [rand(5 * MiB), rand(5 * MiB), rand(KiB)];
    const { UploadId } = await call("createMultipartUpload", { Bucket: B, Key: key });
    const done = [];
    for (let i = 0; i < parts.length; i++) {
      const r = await call("uploadPart", { Bucket: B, Key: key, UploadId, PartNumber: i + 1, Body: parts[i] });
      done.push({ PartNumber: i + 1, ETag: r.ETag });
    }
    const c = await call("completeMultipartUpload", { Bucket: B, Key: key, UploadId, MultipartUpload: { Parts: done } });
    check(/^[0-9a-f]{32}-3$/.test(unquote(c.ETag)), `ETag ${c.ETag} not of form <hex>-3`);
    bufEq((await get(key)).body, Buffer.concat(parts));
  },
  multipart_abort: async () => {
    const key = "mpu/abort";
    const { UploadId } = await call("createMultipartUpload", { Bucket: B, Key: key });
    await call("uploadPart", { Bucket: B, Key: key, UploadId, PartNumber: 1, Body: rand(KiB) });
    await call("abortMultipartUpload", { Bucket: B, Key: key, UploadId });
    await expectError(() => call("listParts", { Bucket: B, Key: key, UploadId }), { codes: ["NoSuchUpload"], statuses: [404] });
  },
  multipart_list_parts: async () => {
    const key = "mpu/lp", parts = [rand(5 * MiB), rand(KiB)];
    const { UploadId } = await call("createMultipartUpload", { Bucket: B, Key: key });
    try {
      for (let i = 0; i < parts.length; i++) {
        await call("uploadPart", { Bucket: B, Key: key, UploadId, PartNumber: i + 1, Body: parts[i] });
      }
      const r = await call("listParts", { Bucket: B, Key: key, UploadId });
      deepEq((r.Parts ?? []).map((p) => p.PartNumber), [1, 2], "PartNumbers");
      deepEq(r.Parts.map((p) => p.Size), parts.map((p) => p.length), "Sizes");
      deepEq(r.Parts.map((p) => unquote(p.ETag)), parts.map(md5hex), "ETags");
    } finally {
      await call("abortMultipartUpload", { Bucket: B, Key: key, UploadId }).catch(() => {});
    }
  },
  multipart_too_small: async () => {
    const key = "mpu/small";
    const { UploadId } = await call("createMultipartUpload", { Bucket: B, Key: key });
    try {
      const done = [];
      for (let i = 1; i <= 2; i++) {
        const r = await call("uploadPart", { Bucket: B, Key: key, UploadId, PartNumber: i, Body: rand(KiB) });
        done.push({ PartNumber: i, ETag: r.ETag });
      }
      await expectError(() => call("completeMultipartUpload", {
        Bucket: B, Key: key, UploadId, MultipartUpload: { Parts: done },
      }), { codes: ["EntityTooSmall"] });
    } finally {
      await call("abortMultipartUpload", { Bucket: B, Key: key, UploadId }).catch(() => {});
    }
  },
  high_level_upload: async () => {
    const data = rand(20 * MiB);
    await s3.upload({ Bucket: B, Key: "hl/obj", Body: data }, { partSize: 5 * MiB, queueSize: 4 }).promise();
    bufEq((await get("hl/obj")).body, data);
  },
  presigned_get: async () => {
    const data = rand(500);
    await put("ps/get", data);
    const url = await s3.getSignedUrlPromise("getObject", { Bucket: B, Key: "ps/get", Expires: 300 });
    const res = await fetch(url);
    eq(res.status, 200, "HTTP status");
    bufEq(Buffer.from(await res.arrayBuffer()), data);
  },
  presigned_put: async () => {
    const data = rand(500);
    const url = await s3.getSignedUrlPromise("putObject", { Bucket: B, Key: "ps/put", Expires: 300 });
    const res = await fetch(url, { method: "PUT", body: data });
    eq(res.status, 200, `HTTP status (${(await res.text()).slice(0, 200)})`);
    bufEq((await get("ps/put")).body, data);
  },
  checksum_crc32: checksumScenario("CRC32"),
  checksum_crc32c: checksumScenario("CRC32C"),
  checksum_sha1: checksumScenario("SHA1"),
  checksum_sha256: checksumScenario("SHA256"),
  checksum_crc64nvme: skip("aws-sdk v2 API model has no CRC64NVME checksum (ChecksumCRC64NVME member absent)"),
  content_md5_bad: () => expectError(
    () => put("md5bad", Buffer.from("payload"), { ContentMD5: Buffer.from(md5hex("other"), "hex").toString("base64") }),
    { codes: ["BadDigest"], statuses: [400] }),
  bad_credentials: () => expectError(() => call("listBuckets", {}, mk("wrong-secret")),
    { codes: ["SignatureDoesNotMatch", "InvalidAccessKeyId"], statuses: [403] }),
  delete_bucket_not_empty: async () => {
    await put("nonempty", Buffer.from("x"));
    await expectError(() => call("deleteBucket", { Bucket: B }), { codes: ["BucketNotEmpty"], statuses: [409] });
  },
  versioning: async () => {
    await call("createBucket", { Bucket: VB });
    await call("putBucketVersioning", { Bucket: VB, VersioningConfiguration: { Status: "Enabled" } });
    const v1 = Buffer.from("version-1"), v2 = Buffer.from("version-2");
    const p1 = await call("putObject", { Bucket: VB, Key: "k", Body: v1 });
    const p2 = await call("putObject", { Bucket: VB, Key: "k", Body: v2 });
    check(p1.VersionId && p2.VersionId && p1.VersionId !== p2.VersionId, `distinct VersionIds expected, got ${p1.VersionId} / ${p2.VersionId}`);
    const l = await call("listObjectVersions", { Bucket: VB, Prefix: "k" });
    const vs = (l.Versions ?? []).filter((v) => v.Key === "k");
    eq(vs.length, 2, "version count");
    deepEq(vs.map((v) => v.VersionId).sort(), [p1.VersionId, p2.VersionId].sort(), "VersionIds");
    bufEq((await get("k", { VersionId: p1.VersionId }, VB)).body, v1, "old version body");
    bufEq((await get("k", {}, VB)).body, v2, "latest body");
  },
  object_tagging: async () => {
    await put("tag.txt", Buffer.from("t"));
    const TagSet = [{ Key: "k1", Value: "v1" }, { Key: "k2", Value: "v 2" }];
    await call("putObjectTagging", { Bucket: B, Key: "tag.txt", Tagging: { TagSet } });
    const r = await call("getObjectTagging", { Bucket: B, Key: "tag.txt" });
    deepEq([...(r.TagSet ?? [])].sort((a, b) => a.Key.localeCompare(b.Key)).map(({ Key, Value }) => ({ Key, Value })), TagSet, "TagSet");
  },
  delete_bucket: async () => {
    await call("createBucket", { Bucket: DB });
    for (const Key of ["x", "y/z"]) await call("putObject", { Bucket: DB, Key, Body: Key });
    const l = await call("listObjectsV2", { Bucket: DB });
    await call("deleteObjects", { Bucket: DB, Delete: { Objects: (l.Contents ?? []).map(({ Key }) => ({ Key })) } });
    await call("deleteBucket", { Bucket: DB });
    await expectError(() => call("headBucket", { Bucket: DB }), NOT_FOUND);
  },

};

await runSuite({ sdk: SDK, version: pkgVersion("aws-sdk"), bucket: B, impl, cleanupBuckets: [VB, DB] });
