# fakes3-sdk

[English](README.md) | 简体中文

[fakes3](https://github.com/AkaraChen/fakes3) 的 Node.js 绑定（[napi-rs](https://napi.rs) 插件）：一个专门给测试用的 S3 服务，全内存、无持久化，
直接跑在 Node 进程里（不起子进程、不需要 Docker）。每个服务有独立的数据、时钟和端口，并行的测试互不干扰。

```bash
npm install --save-dev fakes3-sdk
```

```ts
import { start } from "fakes3-sdk";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 默认监听 127.0.0.1 的随机端口
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

`start(options?)` 返回 `Promise<FakeS3Server>`，所有选项都可省略：

| 选项 | 默认值 | 说明 |
|---|---|---|
| `host` | `127.0.0.1` | 监听地址 |
| `port` | `0` | 监听端口，`0` 表示随机端口 |
| `accessKey` / `secretKey` | `fakes3` / `fakes3secret` | 访问凭证，只在 `auth: true` 时用于校验 |
| `users` | 无 | 更多账户 `[{ name, accessKey, secretKey, accountId?, canonicalId? }]`，见[多用户](../../docs/multi-user.md)；`server.users` 返回完整列表（含生成的 id） |
| `auth` | `false` | 校验签名。默认不校验：任意 access key 和 secret 签名的请求都接受（身份由 access key 决定，没配置过的 key 是默认账户 `fakes3`，即 owner），未签名的是匿名请求（见[认证模式](../../docs/auth.md)）。原来的 `noAuth` 选项已删除，它现在就是默认行为 |
| `inlineThreshold` | `1048576` | Linux 上达到这个大小的对象存进 memfd |
| `requestLog` / `requestLogCapacity` | `true` / `10000` | 是否记录 S3 请求（见下面的控制面），以及最多保留多少条 |
| `domains` | 无 | 虚拟主机寻址的基础域名（`<bucket>.<domain>`），path-style 始终可用 |
| `bucketDefaults` | `"legacy"` | 新建 bucket 的初始设置：`"legacy"` 或 `"aws"`，见[新建 bucket 的默认值](../../docs/bucket-defaults.md) |
| `memoryLimit` | `"auto"` | 对象数据占用内存的上限：字节数，或 `"512MiB"`、`"2GiB"`、`"unlimited"`、`"auto"`；超出时写入返回 HTTP 507 |

`FakeS3Server` 有只读属性 `endpoint`、`host`、`port`、`accessKey`、`secretKey`，以及方法 `memory(): { used, limit }`（当前用量和上限，字节；无上限时没有 `limit` 字段），`stop(): Promise<void>`（可重复调用，调用后端口立即释放）。
类型声明由 `napi build` 生成。

**测试控制面**：[控制面](../../docs/control-plane.md)的操作也有对应的方法，直接在进程内调用，不走 HTTP。时间一律用 JS 的 `Date`。

```ts
server.reset();                         // 清空所有数据、请求日志和时钟；快照保留（reset({ snapshots: true }) 一并删除）
server.snapshot("seeded");              // 保存当前状态 → { name, created, buckets, objects, uploads }
server.restore("seeded");               // 回滚到快照（快照不会被消耗）；snapshots() 列出，deleteSnapshot(name) 删除
server.memory();                        // { used, limit }

server.clock.freeze(new Date("2030-01-02T03:04:05Z"));   // freeze() 不带参数则冻结在当前时间
server.clock.advance("31d");            // 数字按毫秒，字符串如 "1h30m"、"-2w"；还有 set(date)、unfreeze()、reset()
server.clock.now();                     // Date；isFrozen()、isShifted()

server.requests.list({ operation: "PutObject", status: "error", limit: 10 }); // 过滤条件都可省略
server.requests.count({ bucket: "demo" });   // 匹配的条数（忽略 limit）
server.requests.clear();                // configure({ enabled, capacity }) 开关日志、改容量
```

`requests.list()` 返回按到达顺序排列的记录：`{ seq, time, method, path, bucket, key, operation, status, latencyUs, bytesIn, bytesOut, accessKey, requestId }`。
过滤条件：`bucket`、`keyPrefix`、`operation`、`method`、`status`（`404`、`"4xx"` 或 `"error"`）、`since`、`after`（`seq`）、`limit`（只保留最近的几条）。
和控制面一样，这些调用没有和进行中的请求同步，请在测试之间、没有未完成请求时使用。服务停止后调用会抛出 `server is stopped`。

## 平台

预编译产物覆盖 Linux 的 x64 / arm64（glibc ≥ 2.35 和 musl（Alpine）都有）、Apple silicon 和 Intel 的 macOS，以及 x64 的 Windows，
通过 `optionalDependencies` 里的 `@akarachen/fakes3-sdk-linux-x64-gnu`、`@akarachen/fakes3-sdk-darwin-arm64`、`@akarachen/fakes3-sdk-win32-x64-msvc` 等平台包分发，
需要 Node.js 18 或更新版本。

## 安全提示

fakes3 是给测试用的：默认不校验签名，不要把服务暴露在不可信的网络上，也不要拿它存真实数据。不传 `host` 时只监听 `127.0.0.1`。

## 自己编译

```bash
cd bindings/node
npm ci && npm run build   # 需要 Rust 工具链、perl 和 make
npm test                  # 用 @aws-sdk/client-s3 跑一遍
```

## 许可证

MIT
