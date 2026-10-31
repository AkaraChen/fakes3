// Shared runner for the JS SDK matrix: config, timing, result JSON, assertions,
// checksum helpers and best-effort cleanup (via @aws-sdk/client-s3).
import { createHash, randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  S3Client, CreateBucketCommand, HeadBucketCommand, ListObjectVersionsCommand,
  DeleteObjectsCommand, ListMultipartUploadsCommand, AbortMultipartUploadCommand,
  DeleteBucketCommand,
} from "@aws-sdk/client-s3";

const HERE = dirname(fileURLToPath(import.meta.url));

export const cfg = {
  endpoint: process.env.S3_ENDPOINT || "http://127.0.0.1:9100",
  accessKey: process.env.S3_ACCESS_KEY || "fakes3",
  secretKey: process.env.S3_SECRET_KEY || "fakes3secret",
  region: process.env.S3_REGION || "us-east-1",
  server: process.env.MATRIX_SERVER || "unknown",
  out: process.env.MATRIX_OUT || join(HERE, "..", "results", process.env.MATRIX_SERVER || "unknown"),
};

export const TIMEOUT_MS = 60_000;
export const KiB = 1024;
export const MiB = 1024 * 1024;

export function pkgVersion(name) {
  return JSON.parse(readFileSync(join(HERE, "node_modules", name, "package.json"), "utf8")).version;
}

export function uniqueBucket(sdk) {
  return `m-${sdk}-${randomBytes(4).toString("hex")}`.toLowerCase();
}

export const rand = (n) => randomBytes(n);
export const md5hex = (b) => createHash("md5").update(b).digest("hex");
export const unquote = (s) => (s == null ? s : String(s).replace(/^"+|"+$/g, ""));

export class AssertionError extends Error {
  constructor(msg) { super(msg); this.name = "AssertionError"; }
}
export function check(cond, msg) { if (!cond) throw new AssertionError(msg); }
export function eq(actual, expected, what) {
  if (actual !== expected) {
    throw new AssertionError(`${what}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
export function bufEq(actual, expected, what = "body") {
  const a = Buffer.from(actual);
  if (!a.equals(Buffer.from(expected))) {
    throw new AssertionError(`${what} mismatch (got ${a.length} bytes, expected ${expected.length})`);
  }
}
// Order-insensitive for object keys (header/metadata order is not significant), order-sensitive for arrays.
const canon = (v) => (Array.isArray(v) ? v.map(canon)
  : v && typeof v === "object" && !Buffer.isBuffer(v)
    ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])])) : v);
export function deepEq(actual, expected, what) {
  const a = JSON.stringify(canon(actual)), e = JSON.stringify(canon(expected));
  if (a !== e) throw new AssertionError(`${what}: expected ${e}, got ${a}`);
}

/** Normalized {code, status} from any SDK's error object. */
export function errInfo(e) {
  const status = e?.$metadata?.httpStatusCode ?? e?.$response?.statusCode ?? e?.statusCode ?? e?.status
    ?? e?.$response?.status;
  const code = e?.Code ?? e?.code ?? e?.name;
  return { code: code == null ? "" : String(code), status: status == null ? undefined : Number(status) };
}

/** Run fn; it must throw an error whose code is in `codes` or status is in `statuses`. */
export async function expectError(fn, { codes = [], statuses = [] }, what = "operation") {
  let err;
  try { await fn(); } catch (e) { err = e; }
  if (!err) throw new AssertionError(`${what}: expected error ${[...codes, ...statuses].join("/")}, but it succeeded`);
  if (err instanceof AssertionError) throw err;
  const { code, status } = errInfo(err);
  if (codes.includes(code) || (status !== undefined && statuses.includes(status))) return err;
  throw new AssertionError(`${what}: expected ${[...codes, ...statuses].join("/")}, got code=${code} status=${status}: ${firstLine(err?.message)}`);
}

function firstLine(s) { return String(s ?? "").split("\n")[0]; }

export function fmtError(e) {
  let type = e?.name || e?.constructor?.name || "Error";
  const code = e?.Code ?? e?.code;
  if (typeof code === "string" && code && code !== type) type = `${type}(${code})`;
  let msg = firstLine(e?.message ?? e);
  if (!msg) {
    const { status } = errInfo(e);
    if (status) msg = `HTTP ${status}`;
  }
  return `${type}: ${msg}`.slice(0, 300);
}

function withTimeout(p, ms) {
  let t;
  return Promise.race([
    p.finally(() => clearTimeout(t)),
    new Promise((_, rej) => { t = setTimeout(() => rej(Object.assign(new Error(`timed out after ${ms / 1000}s`), { name: "Timeout" })), ms); }),
  ]);
}

export const SCENARIOS = [
  "create_bucket", "head_bucket", "head_bucket_missing", "list_buckets",
  "put_get_small", "put_get_empty", "put_get_large", "special_keys", "head_object_metadata",
  "get_range", "get_range_invalid", "get_missing_key", "get_if_none_match", "get_if_match_fail",
  "put_if_none_match", "delete_object", "delete_missing_object", "delete_objects",
  "list_v2_basic", "list_v2_prefix_delimiter", "list_v2_pagination", "list_v2_start_after", "list_v1",
  "copy_object", "copy_object_replace_metadata",
  "multipart_upload", "multipart_abort", "multipart_list_parts", "multipart_too_small",
  "high_level_upload", "presigned_get", "presigned_put",
  "checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme",
  "content_md5_bad", "bad_credentials", "delete_bucket_not_empty", "versioning", "object_tagging",
  "delete_bucket",
];

/** Marker for scenarios the SDK has no API for. */
export const skip = (reason) => ({ skip: reason });

export function adminClient() {
  return new S3Client({
    endpoint: cfg.endpoint, region: cfg.region, forcePathStyle: true,
    credentials: { accessKeyId: cfg.accessKey, secretAccessKey: cfg.secretKey },
  });
}

export async function ensureBucket(name) {
  const s3 = adminClient();
  try { await s3.send(new HeadBucketCommand({ Bucket: name })); }
  catch { await s3.send(new CreateBucketCommand({ Bucket: name })).catch(() => {}); }
}

/** Best-effort: delete all versions/objects, abort uploads, delete the bucket. */
export async function nukeBucket(name) {
  const s3 = adminClient();
  try {
    for (;;) {
      const r = await s3.send(new ListMultipartUploadsCommand({ Bucket: name }));
      for (const u of r.Uploads ?? []) {
        await s3.send(new AbortMultipartUploadCommand({ Bucket: name, Key: u.Key, UploadId: u.UploadId })).catch(() => {});
      }
      if (!r.IsTruncated) break;
    }
  } catch {}
  for (let i = 0; i < 50; i++) {
    let objs = [];
    try {
      const r = await s3.send(new ListObjectVersionsCommand({ Bucket: name }));
      objs = [...(r.Versions ?? []), ...(r.DeleteMarkers ?? [])].map((v) => ({ Key: v.Key, VersionId: v.VersionId }));
    } catch {
      try {
        const { ListObjectsV2Command } = await import("@aws-sdk/client-s3");
        const r = await s3.send(new ListObjectsV2Command({ Bucket: name }));
        objs = (r.Contents ?? []).map((o) => ({ Key: o.Key }));
      } catch { break; }
    }
    if (!objs.length) break;
    for (let j = 0; j < objs.length; j += 1000) {
      await s3.send(new DeleteObjectsCommand({ Bucket: name, Delete: { Objects: objs.slice(j, j + 1000), Quiet: true } })).catch(() => {});
    }
  }
  await s3.send(new DeleteBucketCommand({ Bucket: name })).catch(() => {});
}

/**
 * Run every scenario in SCENARIOS order. `impl` maps scenario id -> async fn | skip(reason).
 * Missing ids are reported as skip ("not implemented") so the output is always complete.
 */
export async function runSuite({ sdk, version, bucket, impl, setup, cleanupBuckets = [] }) {
  const results = [];
  if (setup) await setup();
  for (const id of SCENARIOS) {
    const s = impl[id];
    if (!s || s.skip) {
      results.push({ scenario: id, status: "skip", error: s?.skip ?? "not implemented for this SDK" });
      console.error(`[${sdk}] ${id}: skip`);
      continue;
    }
    const t0 = performance.now();
    let r;
    try {
      await withTimeout(Promise.resolve().then(s), TIMEOUT_MS);
      r = { scenario: id, status: "pass", ms: +(performance.now() - t0).toFixed(1) };
    } catch (e) {
      r = { scenario: id, status: "fail", ms: +(performance.now() - t0).toFixed(1), error: fmtError(e) };
    }
    results.push(r);
    console.error(`[${sdk}] ${id}: ${r.status}${r.error ? " - " + r.error : ""}`);
    // Later scenarios need the bucket even if the SDK under test failed to create it.
    if (id === "create_bucket") await ensureBucket(bucket);
  }
  for (const b of [bucket, ...cleanupBuckets]) await nukeBucket(b).catch(() => {});
  mkdirSync(cfg.out, { recursive: true });
  const doc = { sdk, lang: "js", version, server: cfg.server, results };
  writeFileSync(join(cfg.out, `${sdk}.json`), JSON.stringify(doc, null, 2) + "\n");
  const n = (st) => results.filter((x) => x.status === st).length;
  console.log(`${sdk} ${version} on ${cfg.server}: pass=${n("pass")} fail=${n("fail")} skip=${n("skip")}`);
}

// ---------- checksums (computed independently of the SDKs) ----------

function crc32Table(poly) {
  const t = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let c = i;
    for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ poly : c >>> 1;
    t[i] = c >>> 0;
  }
  return t;
}
const T32 = crc32Table(0xedb88320), T32C = crc32Table(0x82f63b78);
function crc32With(t, buf) {
  let c = 0xffffffff;
  for (const b of buf) c = t[(c ^ b) & 0xff] ^ (c >>> 8);
  c = (c ^ 0xffffffff) >>> 0;
  const out = Buffer.alloc(4); out.writeUInt32BE(c); return out;
}
const T64 = (() => {
  const poly = 0x9a6c9329ac4bc9b5n, mask = (1n << 64n) - 1n, t = new Array(256);
  for (let i = 0; i < 256; i++) {
    let c = BigInt(i);
    for (let k = 0; k < 8; k++) c = c & 1n ? (c >> 1n) ^ poly : c >> 1n;
    t[i] = c & mask;
  }
  return t;
})();
function crc64nvme(buf) {
  const mask = (1n << 64n) - 1n;
  let c = mask;
  for (const b of buf) c = T64[Number((c ^ BigInt(b)) & 0xffn)] ^ (c >> 8n);
  c = (c ^ mask) & mask;
  const out = Buffer.alloc(8); out.writeBigUInt64BE(c); return out;
}

/** Base64 checksum as S3 returns it in x-amz-checksum-<alg>. */
export function checksumB64(alg, buf) {
  switch (alg) {
    case "CRC32": return crc32With(T32, buf).toString("base64");
    case "CRC32C": return crc32With(T32C, buf).toString("base64");
    case "CRC64NVME": return crc64nvme(buf).toString("base64");
    case "SHA1": return createHash("sha1").update(buf).digest("base64");
    case "SHA256": return createHash("sha256").update(buf).digest("base64");
  }
  throw new Error(`unknown alg ${alg}`);
}

// Sanity-check the hand-rolled CRCs against the published check values.
{
  const v = Buffer.from("123456789");
  if (crc32With(T32, v).toString("hex") !== "cbf43926" || crc32With(T32C, v).toString("hex") !== "e3069283"
      || crc64nvme(v).toString("hex") !== "ae8b14860a799888") throw new Error("checksum self-test failed");
}

export const SPECIAL_KEYS = ["sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"];
export const META = { color: "Blue", "x-y": "z" };
export async function streamToBuffer(s) {
  const chunks = [];
  for await (const c of s) chunks.push(Buffer.from(c));
  return Buffer.concat(chunks);
}
