// minio-js
import * as Minio from "minio";
import { Readable } from "node:stream";
import {
  cfg, runSuite, uniqueBucket, pkgVersion, rand, md5hex, unquote, check, eq, bufEq, deepEq, expectError,
  skip, streamToBuffer, SPECIAL_KEYS, META, KiB, MiB,
} from "./common.mjs";

const SDK = "minio-js";
const u = new URL(cfg.endpoint);
const mk = (secret = cfg.secretKey) => new Minio.Client({
  endPoint: u.hostname, port: Number(u.port || (u.protocol === "https:" ? 443 : 80)), useSSL: u.protocol === "https:",
  accessKey: cfg.accessKey, secretKey: secret, region: cfg.region, pathStyle: true,
});
const mc = mk();
const B = uniqueBucket("miniojs");
const VB = `${B}-v`;
const DB = `${B}-d`; // dedicated bucket for delete_bucket

const put = (key, buf, meta = {}, bucket = B) => mc.putObject(bucket, key, buf, buf.length, meta);
async function get(key, opts, bucket = B) {
  const s = await mc.getObject(bucket, key, opts);
  return { headers: s.headers ?? {}, body: await streamToBuffer(s) };
}
async function getRange(key, offset, length) {
  const s = await mc.getPartialObject(B, key, offset, length);
  return { headers: s.headers ?? {}, body: await streamToBuffer(s) };
}
async function listV2(prefix, { token = "", delimiter = "", maxKeys = 1000, startAfter = "" } = {}) {
  return mc.listObjectsV2Query(B, prefix, token, delimiter, maxKeys, startAfter);
}
async function listAll(prefix = "") {
  const keys = [];
  let token = "";
  for (;;) {
    const r = await listV2(prefix, { token });
    keys.push(...r.objects.filter((o) => o.name !== undefined).map((o) => o.name));
    if (!r.isTruncated) return keys;
    token = r.nextContinuationToken;
  }
}
const NOT_FOUND = { codes: ["NotFound", "NoSuchKey"], statuses: [404] };
const PRECOND = { codes: ["PreconditionFailed"], statuses: [412] };

const impl = {
  create_bucket: () => mc.makeBucket(B, cfg.region),
  head_bucket: async () => eq(await mc.bucketExists(B), true, "bucketExists"),
  head_bucket_missing: async () => eq(await mc.bucketExists(`${B}-missing`), false, "bucketExists(missing)"),
  list_buckets: async () => {
    const r = await mc.listBuckets();
    check(r.some((b) => b.name === B), `bucket ${B} not in listBuckets`);
  },
  put_get_small: async () => {
    const data = Buffer.from("hello world");
    const p = await put("small.txt", data);
    bufEq((await get("small.txt")).body, data);
    eq(unquote(p.etag), md5hex(data), "PUT etag");
    eq(unquote((await mc.statObject(B, "small.txt")).etag), md5hex(data), "stat etag");
  },
  put_get_empty: async () => {
    await put("empty", Buffer.alloc(0));
    const { headers, body } = await get("empty");
    eq(body.length, 0, "body length");
    eq(headers["content-length"], "0", "Content-Length");
  },
  put_get_large: async () => {
    const data = rand(8 * MiB);
    await mc.putObject(B, "large.bin", data, data.length); // below default 64 MiB partSize -> single PUT
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
      "Content-Type": "text/plain", "Cache-Control": "max-age=60", "Content-Disposition": 'attachment; filename="m.txt"',
      "Content-Encoding": "gzip", ...META,
    });
    const m = (await mc.statObject(B, "meta.txt")).metaData;
    eq(m["content-type"], "text/plain", "content-type");
    eq(m["cache-control"], "max-age=60", "cache-control");
    eq(m["content-disposition"], 'attachment; filename="m.txt"', "content-disposition");
    eq(m["content-encoding"], "gzip", "content-encoding");
    eq(m.color, "Blue", "meta color");
    eq(m["x-y"], "z", "meta x-y");
  },
  // minio-js exposes ranges only as (offset, length); a suffix range (bytes=-5) cannot be expressed.
  get_range: async () => {
    const data = rand(100);
    await put("range.bin", data);
    for (const [offset, length, start, end] of [[10, 10, 10, 19], [95, 0, 95, 99]]) {
      const { headers, body } = await getRange("range.bin", offset, length);
      bufEq(body, data.subarray(start, end + 1), `body for offset=${offset}`);
      eq(headers["content-range"], `bytes ${start}-${end}/100`, `Content-Range for offset=${offset}`);
    }
  },
  get_range_invalid: async () => {
    await put("range-inv.bin", rand(100));
    await expectError(() => getRange("range-inv.bin", 200, 101), { codes: ["InvalidRange"], statuses: [416] });
  },
  get_missing_key: () => expectError(() => get("does/not/exist"), NOT_FOUND),
  get_if_none_match: skip("minio-js getObject has no conditional-request (If-None-Match) option"),
  get_if_match_fail: skip("minio-js getObject has no conditional-request (If-Match) option"),
  put_if_none_match: async () => {
    const key = `inm-${Date.now()}`;
    await put(key, Buffer.from("first"), { "If-None-Match": "*" });
    await expectError(() => put(key, Buffer.from("second"), { "If-None-Match": "*" }), PRECOND);
    bufEq((await get(key)).body, Buffer.from("first"));
  },
  delete_object: async () => {
    await put("del.txt", Buffer.from("x"));
    await mc.removeObject(B, "del.txt");
    await expectError(() => mc.statObject(B, "del.txt"), NOT_FOUND);
  },
  delete_missing_object: () => mc.removeObject(B, "never-existed"),
  delete_objects: async () => {
    const keys = ["dm/1", "dm/2", "dm/3"];
    for (const k of keys) await put(k, Buffer.from(k));
    const r = await mc.removeObjects(B, keys);
    const errs = (r ?? []).filter((x) => x && (x.Code || x.code));
    eq(errs.length, 0, `errors ${JSON.stringify(errs)}`);
    deepEq(await listAll("dm/"), [], "remaining keys");
  },
  list_v2_basic: async () => {
    const objs = { "lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7) };
    for (const [k, v] of Object.entries(objs)) await put(k, v);
    const r = await listV2("lb/");
    deepEq(r.objects.map((o) => o.name), ["lb/a", "lb/b", "lb/c"], "keys");
    for (const o of r.objects) {
      eq(o.size, objs[o.name].length, `size of ${o.name}`);
      eq(unquote(o.etag), md5hex(objs[o.name]), `etag of ${o.name}`);
    }
  },
  list_v2_prefix_delimiter: async () => {
    for (const k of ["a/1", "a/2", "b/1", "c"]) await put(`pd/${k}`, Buffer.from(k));
    const r = await listV2("pd/", { delimiter: "/" });
    deepEq(r.objects.filter((o) => o.prefix).map((o) => o.prefix), ["pd/a/", "pd/b/"], "CommonPrefixes");
    deepEq(r.objects.filter((o) => o.name).map((o) => o.name), ["pd/c"], "Contents");
  },
  list_v2_pagination: async () => {
    const keys = Array.from({ length: 25 }, (_, i) => `pg/${String(i).padStart(2, "0")}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const pages = [];
    let token = "";
    for (;;) {
      const r = await listV2("pg/", { token, maxKeys: 10 });
      pages.push(r.objects.map((o) => o.name));
      check(pages.length <= 5, "too many pages");
      if (!r.isTruncated) break;
      token = r.nextContinuationToken;
    }
    deepEq(pages.map((p) => p.length), [10, 10, 5], "page sizes");
    deepEq(pages.flat(), keys, "keys");
  },
  list_v2_start_after: async () => {
    const keys = Array.from({ length: 10 }, (_, i) => `sa/${i}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const r = await listV2("sa/", { startAfter: "sa/4" });
    deepEq(r.objects.map((o) => o.name), keys.slice(5), "keys");
  },
  list_v1: async () => {
    const keys = Array.from({ length: 6 }, (_, i) => `l1/${i}`);
    await Promise.all(keys.map((k) => put(k, Buffer.from(k))));
    const r1 = await mc.listObjectsQuery(B, "l1/", "l1/2", { Delimiter: "", MaxKeys: 2 });
    deepEq(r1.objects.map((o) => o.name), ["l1/3", "l1/4"], "page 1");
    eq(!!r1.isTruncated, true, "isTruncated");
    const r2 = await mc.listObjectsQuery(B, "l1/", "l1/4", { Delimiter: "", MaxKeys: 1000 });
    deepEq(r2.objects.map((o) => o.name), ["l1/5"], "page 2");
    eq(!!r2.isTruncated, false, "isTruncated page 2");
  },
  copy_object: async () => {
    const data = rand(1234);
    await put("cp/src", data, { "Content-Type": "application/x-test", ...META });
    await mc.copyObject(new Minio.CopySourceOptions({ Bucket: B, Object: "cp/src" }),
      new Minio.CopyDestinationOptions({ Bucket: B, Object: "cp/dst" }));
    bufEq((await get("cp/dst")).body, data);
    const m = (await mc.statObject(B, "cp/dst")).metaData;
    eq(m["content-type"], "application/x-test", "content-type");
    eq(m.color, "Blue", "meta color");
    eq(m["x-y"], "z", "meta x-y");
  },
  copy_object_replace_metadata: async () => {
    const data = rand(100);
    await put("cpr/src", data, { "Content-Type": "application/x-old", ...META });
    await mc.copyObject(new Minio.CopySourceOptions({ Bucket: B, Object: "cpr/src" }),
      new Minio.CopyDestinationOptions({
        Bucket: B, Object: "cpr/dst", MetadataDirective: "REPLACE",
        UserMetadata: { fresh: "yes" }, Headers: { "Content-Type": "application/x-new" },
      }));
    const m = (await mc.statObject(B, "cpr/dst")).metaData;
    eq(m["content-type"], "application/x-new", "content-type");
    eq(m.fresh, "yes", "meta fresh");
    eq(m.color, undefined, "old meta color");
    bufEq((await get("cpr/dst")).body, data);
  },
  multipart_upload: async () => {
    const key = "mpu/obj", parts = [rand(5 * MiB), rand(5 * MiB), rand(KiB)];
    const uploadID = await mc.initiateNewMultipartUpload(B, key, {});
    const etags = [];
    for (let i = 0; i < parts.length; i++) {
      const r = await mc.uploadPart({ bucketName: B, objectName: key, uploadID, partNumber: i + 1, headers: {} }, parts[i]);
      etags.push({ part: i + 1, etag: r.etag });
    }
    const c = await mc.completeMultipartUpload(B, key, uploadID, etags);
    check(/^[0-9a-f]{32}-3$/.test(unquote(c.etag)), `etag ${c.etag} not of form <hex>-3`);
    bufEq((await get(key)).body, Buffer.concat(parts));
  },
  multipart_abort: async () => {
    const key = "mpu/abort";
    const uploadID = await mc.initiateNewMultipartUpload(B, key, {});
    await mc.uploadPart({ bucketName: B, objectName: key, uploadID, partNumber: 1, headers: {} }, rand(KiB));
    await mc.abortMultipartUpload(B, key, uploadID);
    await expectError(() => mc.listParts(B, key, uploadID), { codes: ["NoSuchUpload"], statuses: [404] });
  },
  multipart_list_parts: async () => {
    const key = "mpu/lp", parts = [rand(5 * MiB), rand(KiB)];
    const uploadID = await mc.initiateNewMultipartUpload(B, key, {});
    try {
      for (let i = 0; i < parts.length; i++) {
        await mc.uploadPart({ bucketName: B, objectName: key, uploadID, partNumber: i + 1, headers: {} }, parts[i]);
      }
      const r = await mc.listParts(B, key, uploadID);
      deepEq(r.map((p) => p.part), [1, 2], "part numbers");
      deepEq(r.map((p) => p.size), parts.map((p) => p.length), "sizes");
      deepEq(r.map((p) => unquote(p.etag)), parts.map(md5hex), "etags");
    } finally {
      await mc.abortMultipartUpload(B, key, uploadID).catch(() => {});
    }
  },
  multipart_too_small: async () => {
    const key = "mpu/small";
    const uploadID = await mc.initiateNewMultipartUpload(B, key, {});
    try {
      const etags = [];
      for (let i = 1; i <= 2; i++) {
        const r = await mc.uploadPart({ bucketName: B, objectName: key, uploadID, partNumber: i, headers: {} }, rand(KiB));
        etags.push({ part: i, etag: r.etag });
      }
      await expectError(() => mc.completeMultipartUpload(B, key, uploadID, etags), { codes: ["EntityTooSmall"] });
    } finally {
      await mc.abortMultipartUpload(B, key, uploadID).catch(() => {});
    }
  },
  // putObject with a stream larger than partSize transparently does a multipart upload.
  high_level_upload: async () => {
    const data = rand(20 * MiB);
    const client = new Minio.Client({
      endPoint: u.hostname, port: Number(u.port || 80), useSSL: false, accessKey: cfg.accessKey,
      secretKey: cfg.secretKey, region: cfg.region, pathStyle: true, partSize: 5 * MiB,
    });
    const chunks = Array.from({ length: 20 }, (_, i) => data.subarray(i * MiB, (i + 1) * MiB));
    const r = await client.putObject(B, "hl/obj", Readable.from(chunks), data.length);
    check(/-\d+$/.test(unquote(r.etag)), `expected multipart etag, got ${r.etag}`);
    bufEq((await get("hl/obj")).body, data);
  },
  presigned_get: async () => {
    const data = rand(500);
    await put("ps/get", data);
    const res = await fetch(await mc.presignedGetObject(B, "ps/get", 300));
    eq(res.status, 200, "HTTP status");
    bufEq(Buffer.from(await res.arrayBuffer()), data);
  },
  presigned_put: async () => {
    const data = rand(500);
    const res = await fetch(await mc.presignedPutObject(B, "ps/put", 300), { method: "PUT", body: data });
    eq(res.status, 200, `HTTP status (${(await res.text()).slice(0, 200)})`);
    bufEq((await get("ps/put")).body, data);
  },
  checksum_crc32: skip("minio-js has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API"),
  checksum_crc32c: skip("minio-js has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API"),
  checksum_sha1: skip("minio-js has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API"),
  checksum_sha256: skip("minio-js has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API"),
  checksum_crc64nvme: skip("minio-js has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API"),
  content_md5_bad: skip("minio-js putObject has no Content-MD5 option (non-allowlisted metadata keys become x-amz-meta-*)"),
  bad_credentials: () => expectError(() => mk("wrong-secret").listBuckets(),
    { codes: ["SignatureDoesNotMatch", "InvalidAccessKeyId"], statuses: [403] }),
  delete_bucket_not_empty: async () => {
    await put("nonempty", Buffer.from("x"));
    await expectError(() => mc.removeBucket(B), { codes: ["BucketNotEmpty"], statuses: [409] });
  },
  versioning: async () => {
    await mc.makeBucket(VB, cfg.region);
    await mc.setBucketVersioning(VB, { Status: "Enabled" });
    const v1 = Buffer.from("version-1"), v2 = Buffer.from("version-2");
    const p1 = await put("k", v1, {}, VB);
    const p2 = await put("k", v2, {}, VB);
    check(p1.versionId && p2.versionId && p1.versionId !== p2.versionId, `distinct versionIds expected, got ${p1.versionId} / ${p2.versionId}`);
    const vs = [];
    for await (const o of mc.listObjects(VB, "k", true, { IncludeVersion: true })) if (o.name === "k") vs.push(o);
    eq(vs.length, 2, "version count");
    deepEq(vs.map((v) => v.versionId).sort(), [p1.versionId, p2.versionId].sort(), "versionIds");
    bufEq((await get("k", { versionId: p1.versionId }, VB)).body, v1, "old version body");
    bufEq((await get("k", undefined, VB)).body, v2, "latest body");
  },
  object_tagging: async () => {
    await put("tag.txt", Buffer.from("t"));
    await mc.setObjectTagging(B, "tag.txt", { k1: "v1", k2: "v 2" });
    const r = await mc.getObjectTagging(B, "tag.txt");
    deepEq([...r].sort((a, b) => a.Key.localeCompare(b.Key)).map(({ Key, Value }) => ({ Key, Value })),
      [{ Key: "k1", Value: "v1" }, { Key: "k2", Value: "v 2" }], "tags");
  },
  delete_bucket: async () => {
    await mc.makeBucket(DB, cfg.region);
    for (const k of ["x", "y/z"]) await put(k, Buffer.from(k), {}, DB);
    const keys = [];
    for await (const o of mc.listObjectsV2(DB, "", true)) keys.push(o.name);
    await mc.removeObjects(DB, keys);
    await mc.removeBucket(DB);
    eq(await mc.bucketExists(DB), false, "bucketExists after delete");
  },

};

await runSuite({ sdk: SDK, version: pkgVersion("minio"), bucket: B, impl, cleanupBuckets: [VB, DB] });
