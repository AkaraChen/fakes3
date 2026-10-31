// Bun's built-in S3 client (Bun.S3Client). Run with `bun`.
// Bun.S3Client is object-level only: no bucket APIs, no copy, no tagging/versioning,
// no conditional headers, no low-level multipart, no user metadata, no checksums.
// The bucket is created/destroyed via @aws-sdk/client-s3 (setup only).
import {
  cfg, runSuite, uniqueBucket, rand, md5hex, unquote, check, eq, bufEq, deepEq, expectError,
  skip, ensureBucket, SPECIAL_KEYS, KiB, MiB,
} from "./common.mjs";

const SDK = "bun-s3";
const B = uniqueBucket("bun");
const opts = (secret = cfg.secretKey) => ({
  endpoint: cfg.endpoint, region: cfg.region, bucket: B, virtualHostedStyle: false,
  accessKeyId: cfg.accessKey, secretAccessKey: secret, retry: 0,
});
const s3 = new Bun.S3Client(opts());

const put = (key, data, o) => s3.file(key).write(data, o);
const getBytes = async (key) => Buffer.from(await s3.file(key).bytes());
const NOT_FOUND = { codes: ["NoSuchKey", "NotFound", "ERR_S3_FILE_NOT_FOUND"], statuses: [404] };

async function listAll(prefix) {
  const keys = [];
  let continuationToken;
  do {
    const r = await s3.list({ prefix, continuationToken });
    keys.push(...(r.contents ?? []).map((o) => o.key));
    continuationToken = r.isTruncated ? r.nextContinuationToken : undefined;
  } while (continuationToken);
  return keys;
}

const NO_BUCKET = skip("Bun.S3Client has no bucket-level API (create/head/list/delete bucket)");
const NO_COND = skip("Bun.S3Client has no conditional-request headers (If-Match/If-None-Match)");
const NO_MPU = skip("Bun.S3Client has no low-level multipart API (only transparent multipart via writer())");
const NO_CKSUM = skip("Bun.S3Client has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API");

const impl = {
  create_bucket: NO_BUCKET,
  head_bucket: NO_BUCKET,
  head_bucket_missing: NO_BUCKET,
  list_buckets: NO_BUCKET,
  put_get_small: async () => {
    const data = Buffer.from("hello world");
    await put("small.txt", data);
    bufEq(await getBytes("small.txt"), data);
    eq(unquote((await s3.stat("small.txt")).etag), md5hex(data), "ETag");
  },
  put_get_empty: async () => {
    await put("empty", Buffer.alloc(0));
    eq((await getBytes("empty")).length, 0, "body length");
    eq((await s3.stat("empty")).size, 0, "Content-Length (stat size)");
  },
  put_get_large: async () => {
    const data = rand(8 * MiB);
    await put("large.bin", data, { partSize: 16 * MiB }); // partSize > size -> single PUT
    bufEq(await getBytes("large.bin"), data);
    check(!unquote((await s3.stat("large.bin")).etag).includes("-"), "expected a single-PUT ETag");
  },
  special_keys: async () => {
    for (const k of SPECIAL_KEYS) {
      const data = Buffer.from(`data:${k}`);
      await put(k, data);
      bufEq(await getBytes(k), data, `body of ${k}`);
    }
    const keys = await listAll();
    for (const k of SPECIAL_KEYS) check(keys.includes(k), `key ${JSON.stringify(k)} missing from list`);
  },
  head_object_metadata: skip("Bun.S3Client cannot set Cache-Control or user metadata, and stat() exposes only size/etag/type"),
  // Bun exposes ranges via slice(); Content-Range is not surfaced, so only bodies are checked.
  // A suffix range (bytes=-5) is not expressible: slice(-5) on an S3File of unknown size sends
  // bytes=4503599627370490-4503599627370494 (Bun 1.4.3), so it is omitted.
  get_range: async () => {
    const data = rand(100);
    await put("range.bin", data);
    bufEq(Buffer.from(await s3.file("range.bin").slice(10, 20).bytes()), data.subarray(10, 20), "bytes=10-19");
    bufEq(Buffer.from(await s3.file("range.bin").slice(95).bytes()), data.subarray(95), "bytes=95-");
  },
  get_range_invalid: async () => {
    await put("range-inv.bin", rand(100));
    await expectError(async () => {
      const b = await s3.file("range-inv.bin").slice(200, 301).bytes();
      throw new Error(`no error; got ${b.length} bytes`);
    }, { codes: ["InvalidRange"], statuses: [416] });
  },
  get_missing_key: () => expectError(() => getBytes("does/not/exist"), NOT_FOUND),
  get_if_none_match: NO_COND,
  get_if_match_fail: NO_COND,
  put_if_none_match: NO_COND,
  delete_object: async () => {
    await put("del.txt", "x");
    await s3.delete("del.txt");
    eq(await s3.exists("del.txt"), false, "exists after delete");
  },
  delete_missing_object: () => s3.delete("never-existed"),
  delete_objects: skip("Bun.S3Client has no batch DeleteObjects API"),
  list_v2_basic: async () => {
    const objs = { "lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7) };
    for (const [k, v] of Object.entries(objs)) await put(k, v);
    const r = await s3.list({ prefix: "lb/" });
    deepEq(r.contents.map((o) => o.key), ["lb/a", "lb/b", "lb/c"], "keys");
    for (const o of r.contents) {
      eq(o.size, objs[o.key].length, `size of ${o.key}`);
      eq(unquote(o.eTag), md5hex(objs[o.key]), `eTag of ${o.key}`);
    }
  },
  list_v2_prefix_delimiter: async () => {
    for (const k of ["a/1", "a/2", "b/1", "c"]) await put(`pd/${k}`, k);
    const r = await s3.list({ prefix: "pd/", delimiter: "/" });
    deepEq((r.commonPrefixes ?? []).map((p) => p.prefix), ["pd/a/", "pd/b/"], "commonPrefixes");
    deepEq((r.contents ?? []).map((o) => o.key), ["pd/c"], "contents");
  },
  list_v2_pagination: async () => {
    const keys = Array.from({ length: 25 }, (_, i) => `pg/${String(i).padStart(2, "0")}`);
    await Promise.all(keys.map((k) => put(k, k)));
    const pages = [];
    let continuationToken;
    do {
      const r = await s3.list({ prefix: "pg/", maxKeys: 10, continuationToken });
      pages.push((r.contents ?? []).map((o) => o.key));
      continuationToken = r.isTruncated ? r.nextContinuationToken : undefined;
      check(pages.length <= 5, "too many pages");
    } while (continuationToken);
    deepEq(pages.map((p) => p.length), [10, 10, 5], "page sizes");
    deepEq(pages.flat(), keys, "keys");
  },
  list_v2_start_after: async () => {
    const keys = Array.from({ length: 10 }, (_, i) => `sa/${i}`);
    await Promise.all(keys.map((k) => put(k, k)));
    const r = await s3.list({ prefix: "sa/", startAfter: "sa/4" });
    deepEq((r.contents ?? []).map((o) => o.key), keys.slice(5), "keys");
  },
  list_v1: skip("Bun.S3Client only implements ListObjectsV2"),
  copy_object: skip("Bun.S3Client has no CopyObject API"),
  copy_object_replace_metadata: skip("Bun.S3Client has no CopyObject API"),
  // writer() is Bun's only multipart path; with partSize=5 MiB, 5 MiB + 5 MiB + 1 KiB -> 3 parts.
  multipart_upload: async () => {
    const parts = [rand(5 * MiB), rand(5 * MiB), rand(KiB)];
    const w = s3.file("mpu/obj").writer({ partSize: 5 * MiB, queueSize: 1 });
    for (const p of parts) { w.write(p); await w.flush(); }
    await w.end();
    const etag = unquote((await s3.stat("mpu/obj")).etag);
    check(/^[0-9a-f]{32}-3$/.test(etag), `ETag ${etag} not of form <hex>-3`);
    bufEq(await getBytes("mpu/obj"), Buffer.concat(parts));
  },
  multipart_abort: NO_MPU,
  multipart_list_parts: NO_MPU,
  multipart_too_small: NO_MPU,
  high_level_upload: async () => {
    const data = rand(20 * MiB);
    await put("hl/obj", data, { partSize: 5 * MiB });
    bufEq(await getBytes("hl/obj"), data);
  },
  presigned_get: async () => {
    const data = rand(500);
    await put("ps/get", data);
    const res = await fetch(s3.presign("ps/get", { method: "GET", expiresIn: 300 }));
    eq(res.status, 200, "HTTP status");
    bufEq(Buffer.from(await res.arrayBuffer()), data);
  },
  presigned_put: async () => {
    const data = rand(500);
    const res = await fetch(s3.presign("ps/put", { method: "PUT", expiresIn: 300 }), { method: "PUT", body: data });
    eq(res.status, 200, `HTTP status (${(await res.text()).slice(0, 200)})`);
    bufEq(await getBytes("ps/put"), data);
  },
  checksum_crc32: NO_CKSUM,
  checksum_crc32c: NO_CKSUM,
  checksum_sha1: NO_CKSUM,
  checksum_sha256: NO_CKSUM,
  checksum_crc64nvme: NO_CKSUM,
  content_md5_bad: skip("Bun.S3Client has no Content-MD5 option"),
  // No ListBuckets in Bun; the closest signed call is ListObjectsV2 on our bucket.
  bad_credentials: () => expectError(() => new Bun.S3Client(opts("wrong-secret")).list({ maxKeys: 1 }),
    { codes: ["SignatureDoesNotMatch", "InvalidAccessKeyId"], statuses: [403] }),
  delete_bucket_not_empty: NO_BUCKET,
  versioning: skip("Bun.S3Client has no versioning API"),
  object_tagging: skip("Bun.S3Client has no tagging API"),
  delete_bucket: NO_BUCKET,
};

await runSuite({ sdk: SDK, version: Bun.version, bucket: B, impl, setup: () => ensureBucket(B) });
