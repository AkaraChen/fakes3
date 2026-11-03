# fakes3-sdk

English | [简体中文](https://github.com/AkaraChen/fakes3/blob/main/bindings/node/README.zh-CN.md)

Node.js bindings for [fakes3](https://github.com/AkaraChen/fakes3), an in-memory S3 server for tests.
The server runs inside your Node.js process, with no child process and no Docker. It starts in
milliseconds and behaves like AWS S3 wherever your client can tell the difference.

```bash
npm install --save-dev fakes3-sdk
```

```ts
import { start } from "fakes3-sdk";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 127.0.0.1, random free port
const s3 = new S3Client({
  endpoint: server.endpoint,
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
});
await s3.send(new CreateBucketCommand({ Bucket: "demo" }));
// ...
await server.stop();
```

Every server has its own data, clock and port, so parallel tests don't interfere with each other.

## `start(options?)`

Returns `Promise<FakeS3Server>`. Every option is optional:

| Option | Default | Description |
| --- | --- | --- |
| `host` | `"127.0.0.1"` | Address to listen on |
| `port` | `0` | Port to listen on; `0` picks a free one |
| `accessKey` / `secretKey` | `"fakes3"` / `"fakes3secret"` | Credentials of the default account, checked only with `auth: true` |
| `auth` | `false` | Verify signatures. By default any credentials work: a signed request acts as the account its access key belongs to (the default account if none does), and an unsigned request is anonymous, subject to ACLs, bucket policies and Block Public Access |
| `users` | none | More accounts: `[{ name, accessKey, secretKey, accountId?, canonicalId? }]`. `server.users` lists them all, with generated ids |
| `bucketDefaults` | `"legacy"` | Settings new buckets start with: `"legacy"`, or `"aws"` for what AWS applies today (ACLs disabled, Block Public Access on, SSE-S3, SSE-C blocked) |
| `memoryLimit` | `"auto"` | Cap on object data: bytes, `"512MiB"`, `"2GiB"`, `"unlimited"`, or `"auto"` (75% of the container's or machine's memory). Writes past it fail with HTTP 507 |
| `domains` | none | Base domains for virtual-host addressing (`<bucket>.<domain>`); path-style always works |
| `inlineThreshold` | `1048576` | On Linux, objects at or above this many bytes are stored in a `memfd` instead of on the heap |
| `requestLog` / `requestLogCapacity` | `true` / `10000` | Whether to record requests for `server.requests`, and how many to keep |

## `FakeS3Server`

| Member | Description |
| --- | --- |
| `endpoint`, `host`, `port` | Where the server listens. `endpoint` is `http://host:port`, ready for an S3 client |
| `accessKey`, `secretKey`, `users` | The configured accounts |
| `stop(): Promise<void>` | Stops the server and releases the port immediately. Safe to call more than once |
| `memory(): { used, limit? }` | Object data held, in bytes, and the limit (omitted when unlimited) |
| `reset(options?)` | Clears all buckets, objects and uploads, and by default the request log and clock. Pass `{ snapshots: true }` to delete snapshots too |
| `snapshot(name?)`, `restore(name)`, `snapshots()`, `deleteSnapshot(name)` | Save the current state and roll back to it, as often as you like. Object data is shared, not copied |
| `clock` | The server clock (below) |
| `requests` | The request log (below) |

### Virtual clock

Every timestamp the server produces reads this clock, and so does everything that depends on time:
presigned URL expiry, lifecycle rules, Object Lock retention and archive restores.

```ts
server.clock.freeze(new Date("2030-01-02T03:04:05Z")); // freeze() alone freezes at the current time
server.clock.advance("31d");   // milliseconds, or a string such as "1h30m" or "-2w"
server.clock.set(date);        // jump, keeping the frozen or running state
server.clock.unfreeze();       // let time run again
server.clock.reset();          // back to the host clock
server.clock.now();            // Date; also isFrozen(), isShifted()
```

### Request log

Assert on what your code actually sent:

```ts
server.requests.list({ operation: "PutObject", status: "error", limit: 10 });
server.requests.count({ bucket: "demo" });   // matches, ignoring limit
server.requests.clear();
server.requests.configure({ enabled: true, capacity: 50_000 });
```

Records come in arrival order:
`{ seq, time, method, path, bucket, key, operation, status, latencyUs, bytesIn, bytesOut, accessKey, requestId }`.
Filters: `bucket`, `keyPrefix`, `operation`, `method`, `status` (`404`, `"4xx"` or `"error"`), `since`,
`after` (a `seq`) and `limit` (keep only the most recent matches).

Control calls are not synchronized with in-flight requests, so make them between tests, while no
requests are pending. After `stop()`, they throw `server is stopped`.

## Platforms

Prebuilt binaries are published as optional dependencies, and npm installs the one that matches
your system:

| Package | Platform |
| --- | --- |
| `@akarachen/fakes3-sdk-linux-x64-gnu` | Linux x64, glibc ≥ 2.35 |
| `@akarachen/fakes3-sdk-linux-arm64-gnu` | Linux arm64, glibc ≥ 2.35 |
| `@akarachen/fakes3-sdk-linux-x64-musl` | Linux x64, musl (Alpine) |
| `@akarachen/fakes3-sdk-linux-arm64-musl` | Linux arm64, musl (Alpine) |
| `@akarachen/fakes3-sdk-darwin-arm64` | macOS on Apple silicon |
| `@akarachen/fakes3-sdk-darwin-x64` | macOS on Intel |
| `@akarachen/fakes3-sdk-win32-x64-msvc` | Windows x64 |

Node.js 18 or later.

## Security

fakes3 is for tests. Signatures aren't verified by default, so don't expose the server to an
untrusted network or store real data in it. It listens on `127.0.0.1` unless you pass `host`.

## Building from source

```bash
cd bindings/node
npm ci && npm run build   # needs a Rust toolchain, perl and make
npm test
```

## License

MIT
