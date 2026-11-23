// What a Node test pays for its own fakes3: `start()`, one request, `stop()`.
// Same cycle and output shape as src/main.rs (milliseconds); rendered by report.py.
//
//   npm install && node startup.mjs --out result-node.json
import http from "node:http";
import { writeFileSync } from "node:fs";
import { performance } from "node:perf_hooks";

const argv = process.argv.slice(2);
const arg = (name, def) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : def;
};
const warmup = Number(arg("--warmup", "10"));
const n = Number(arg("--cycles", "200"));
const rounds = Number(arg("--parallel-rounds", "5"));
const sizes = arg("--parallel", "100,500").split(",").filter(Boolean).map(Number);
const out = arg("--out", "");

const t0 = performance.now();
const { start } = await import("fakes3-sdk");
const importMs = performance.now() - t0;

// fakes3 accepts any signature by default, so a well-formed Authorization header is enough.
const headers = {
  authorization:
    "AWS4-HMAC-SHA256 Credential=fakes3/20260101/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature=0000",
  "x-amz-date": "20260101T000000Z",
  "x-amz-content-sha256": "UNSIGNED-PAYLOAD",
};

function listBuckets(server) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: server.host, port: server.port, path: "/", method: "GET", headers, agent: false },
      (res) => {
        res.resume();
        res.on("end", () => (res.statusCode === 200 ? resolve() : reject(new Error(`HTTP ${res.statusCode}`))));
      },
    );
    req.on("error", reject);
    req.end();
  });
}

async function cycle() {
  let t = performance.now();
  const server = await start();
  const startMs = performance.now() - t;
  t = performance.now();
  await listBuckets(server);
  const request = performance.now() - t;
  t = performance.now();
  await server.stop();
  const stop = performance.now() - t;
  return { start: startMs, request, stop, total: startMs + request + stop };
}

// --rss N: only measure the memory N live servers add to a fresh process, then exit.
const rssN = Number(arg("--rss", "0"));
if (rssN) {
  const mib = () => process.memoryUsage().rss / 2 ** 20;
  const base = mib();
  const servers = await Promise.all(Array.from({ length: rssN }, () => start()));
  await Promise.all(servers.map(listBuckets));
  const text = JSON.stringify({ node_rss: { servers: rssN, base_mib: base, added_mib: mib() - base } }, null, 2);
  await Promise.all(servers.map((s) => s.stop()));
  if (out) writeFileSync(out, text);
  else console.log(text);
  process.exit(0);
}

const first = await cycle();
for (let i = 1; i < warmup; i++) await cycle();
const cycles = [];
for (let i = 0; i < n; i++) cycles.push(await cycle());
const col = (k) => cycles.map((c) => c[k]);
const sequential = { first, start: col("start"), request: col("request"), stop: col("stop"), total: col("total") };
console.error("[node] sequential done");

const parallel = {};
for (const size of sizes) {
  parallel[size] = [];
  for (let r = 0; r < rounds; r++) {
    const t = performance.now();
    const totals = (await Promise.all(Array.from({ length: size }, cycle))).map((c) => c.total).sort((a, b) => a - b);
    parallel[size].push({ wall: performance.now() - t, median: totals[size >> 1], max: totals[size - 1] });
  }
  console.error(`[node] parallel ${size} done`);
}

const result = { node: { import: importMs, sequential, parallel }, node_version: process.versions.node };
const text = JSON.stringify(result, null, 2);
if (out) writeFileSync(out, text);
else console.log(text);
