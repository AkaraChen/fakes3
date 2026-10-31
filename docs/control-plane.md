# 测试控制面

fakes3 在 S3 接口所在的同一个端口上提供一组 HTTP API，前缀是 `/_fakes3/`，用来在测试之间重置状态、保存和回滚数据、
控制服务器时间、检查收到过哪些 S3 请求。Bucket 名不能以 `_` 开头，所以 path-style 的 S3 请求不会和这个前缀冲突。

不需要写客户端库：用命令行 `fakes3 ctl`，或者任何能发 HTTP 请求的工具（`curl`）。
Rust 进程内使用时，同样的操作是 `fakes3::Server` 上的方法（见[最后一节](#rust-api)）。

```bash
fakes3 --port 9000 &
export FAKES3_ENDPOINT=http://127.0.0.1:9000     # ctl 的默认值就是这个

fakes3 ctl snapshot create seeded                # 灌好测试数据后存一份
fakes3 ctl reset                                 # 每个测试开始前
fakes3 ctl snapshot restore seeded               # 或者回到存好的状态
fakes3 ctl clock advance 31d                     # 让对象“过去” 31 天
fakes3 ctl requests list --op PutObject --status error
```

## 安全

**控制面没有认证**，不管服务有没有开 SigV4（`--no-auth` 与此无关）。任何连得上端口的人都能清空数据。
这是一个测试服务，默认只监听 `127.0.0.1`；Docker 镜像监听 `0.0.0.0`，请不要把端口暴露到不可信的网络。

唯一的防护：带 `Origin` 头的非 GET 请求会被拒绝（403），防止用户浏览器里打开的网页向 `localhost` 发跨站 POST 把服务清空。
curl、`fakes3 ctl` 和测试框架的 HTTP 客户端不会发 `Origin`。

## 约定

- 请求体和响应体都是 JSON。请求体可以为空（等价于 `{}`），未知字段会报 400。
- 时间用 RFC 3339，响应里是毫秒精度的 UTC，例如 `2030-01-02T03:04:05.000Z`。时间都是**服务器时钟**的时间（见[时钟](#时钟)）。
- 错误响应：`{"error": {"code": "NotFound", "message": "no snapshot named \"x\""}}`，状态码 400 / 403 / 404 / 405 / 409 / 413。
- 控制面自己的请求不记录在请求日志里。
- `fakes3 ctl` 默认打印整理过的文本，`--json` 打印服务器返回的 JSON。出错时往 stderr 写信息，退出码非 0。

## CLI

`fakes3 ctl [--endpoint URL] [--json] <命令>`。`--endpoint` 也可以用环境变量 `FAKES3_ENDPOINT` 指定，默认 `http://127.0.0.1:9000`
（只支持 `http://`）。不带 `ctl` 的 `fakes3 --port 0 ...` 仍然是启动服务。

| 命令 | 说明 |
|---|---|
| `status` | 时钟、内存、数据量、快照数、请求日志状态 |
| `stats` | 内存用量（对象数据字节数和上限）、数据量、快照数、请求日志状态 |
| `reset [--keep-clock] [--keep-requests] [--drop-snapshots]` | 清空数据，见[重置](#重置) |
| `snapshot create [NAME]` | 保存当前状态；不给名字时用 `snap-<n>` |
| `snapshot list` | 列出快照 |
| `snapshot restore NAME` | 回滚到快照 |
| `snapshot delete NAME` | 删除快照 |
| `clock get` | 查看服务器时间 |
| `clock freeze [AT]` | 冻结时间；`AT` 是 RFC 3339 时间，省略则冻结在当前时间 |
| `clock unfreeze` | 从当前服务器时间继续走 |
| `clock set AT` | 跳到指定时间，保持冻结/运行状态 |
| `clock advance DURATION` | 前进（负数为后退），如 `30d`、`1h30m`、`500ms`、`-2w` |
| `clock reset` | 回到主机时钟 |
| `requests list [--bucket B] [--key-prefix P] [--op OP] [--method M] [--status S] [--since T] [--after SEQ] [--limit N] [--count]` | 列出请求日志 |
| `requests clear` | 清空请求日志 |
| `requests config [--enable \| --disable] [--capacity N]` | 查看或修改请求日志设置 |

Duration 的单位：`ns`、`us`、`ms`、`s`、`m`、`h`、`d`（24 小时）、`w`，可以连写（`1h30m`）；不带单位按秒算。
负数可以直接写：`fakes3 ctl clock advance -1d`。

服务端启动参数：`--no-request-log` 关闭请求日志，`--request-log-capacity N`（默认 10000）设置日志容量。

## 重置

`POST /_fakes3/reset`

清空所有 bucket、对象（包括各版本和删除标记）、分段上传，bucket 的版本控制状态和标签也随之消失。默认同时：

- 清空请求日志（序号 `seq` 不归零）；
- 把时钟恢复为主机时钟。

**不会删除快照**，这样“存一份基线，每个测试开始前 reset 或 restore”的用法可以成立。可以用查询参数调整，都是布尔值：

| 参数 | 默认 | 含义 |
|---|---|---|
| `clock` | `true` | 恢复时钟 |
| `requests` | `true` | 清空请求日志 |
| `snapshots` | `false` | 删除所有快照 |

```
$ curl -X POST 'http://127.0.0.1:9000/_fakes3/reset?clock=false'
{"reset":true}
```

访问凭证、监听地址等启动配置不受影响，请求日志的开关和容量也不会被重置。Reset 与正在进行的请求没有同步：已经拿到旧 bucket 的请求会写进一个已被丢弃的 bucket，
所以请在没有未完成请求的时候调用。

## 快照

快照保存所有 bucket 的：创建时间、版本控制状态、标签、ACL / OwnershipControls / PublicAccessBlock / Policy / CORS / Lifecycle / Website / Notification 配置、对象索引（含所有版本和删除标记）、进行中的分段上传（含已上传的 part）。
**对象数据本身不复制**：数据是不可变、带引用计数的分段，快照只增加引用计数，所以快照的代价取决于对象*个数*，而不是字节数。
快照也会让被后来覆盖或删除的数据继续留在内存里，直到快照被删除。

- `POST /_fakes3/snapshots`，请求体 `{"name": "seeded"}`（可省略；名字 1～64 个字符，`A-Za-z0-9._-`）。返回 201。重名返回 409。
- `GET /_fakes3/snapshots` 列出。
- `POST /_fakes3/snapshots/{name}/restore` 回滚。快照不会被消耗，可以反复 restore。
- `DELETE /_fakes3/snapshots/{name}`。

```
$ curl -X POST localhost:9000/_fakes3/snapshots -d '{"name":"seeded"}'
{"name":"seeded","created":"2026-10-31T07:21:30.998Z","buckets":2,"objects":1500,"uploads":1}

$ curl localhost:9000/_fakes3/snapshots
{"snapshots":[{"name":"seeded","created":"2026-10-31T07:21:30.998Z","buckets":2,"objects":1500,"uploads":1}]}

$ curl -X POST localhost:9000/_fakes3/snapshots/seeded/restore
{"name":"seeded","created":"2026-10-31T07:21:30.998Z","buckets":2,"objects":1500,"uploads":1}
```

`objects` 是有版本的 key 的个数（删除标记也算），`uploads` 是进行中的分段上传个数。

**Restore 的语义**

- 数据整体替换为快照时的样子：快照之后新建的 bucket 消失，被删除的 bucket 回来，覆盖/删除的对象恢复。
- 不影响：时钟、请求日志、其他快照、启动配置。
- 快照和 restore 没有和正在进行的请求同步。快照是按 bucket 各自加锁复制的，所以在有请求并发写入时，每个 bucket 内部一致，
  但跨 bucket 不保证（例如跨 bucket 的 CopyObject 正在进行）。**请在测试之间、没有未完成请求时使用。**
- 快照存在内存里，随服务进程消失，不能导出。

### 快照的实现和选型

快照复制的是每个 bucket 的索引（`BTreeMap<key, 版本链>`）：每个 key 一次 `String` 和一个小 `Vec<Arc<Version>>` 的克隆，
不碰对象数据。快照期间持有 bucket 的读锁，大索引上写入会被阻塞到复制完成（100 万个对象时约 30 ms）。比较过两种做法，在 GitHub Actions（4 核托管 runner）上用 `snapshot_scale` 测试（`cargo test --release snapshot_scale -- --ignored --nocapture`，
同一个 bucket，key 形如 `prefix/00000042`，5 次取最好）测得：

| 对象数 | 方案 A：复制 `BTreeMap`（采用） 快照 / restore | 方案 B：`imbl::OrdMap` 快照 / restore | 构造并插入一个对象：A / B |
|---:|---:|---:|---:|
| 1,000 | 14 µs / 45 µs | 0.09 µs / 42 µs | 253 ns / 568 ns |
| 100,000 | 2.6 ms / 4.4 ms | 0.08 µs / 0.22 ms | 239 ns / 637 ns |
| 1,000,000 | 31 ms / 47 ms | 0.11 µs / 0.34 ms | 265 ns / 689 ns |

- 方案 B 的快照是 O(1)，restore 也几乎是 O(1)；方案 A 的快照和 restore 是 O(对象个数)，100 万个对象时是几十毫秒。
- 方案 B 的代价落在每次普通写入上：“构造一个对象并插入索引”这一整步慢 2.2～2.6 倍（上表最后一列，其中索引插入之外的部分对两种方案相同，
  所以索引插入本身的差距更大）。在同一台 runner 上交替跑三轮 bench（4 核，每轮 15 s）：方案 B 的 PUT、64 并发 PUT、GET、版本控制混合负载的吞吐
  比方案 A 低 1.3%～1.6%，模拟测试套件慢 0.8%，三轮方向一致、轮间波动小于 1%；它还把 `imbl`（及其依赖）带进二进制，
  并让所有直接操作索引的代码（`range`、`entry` 等）依赖另一套 API。
- 测试服务器的典型规模是几十到几万个对象，快照用得很少（每个测试一次，或每个套件一次），在这个规模上方案 A 的快照不超过几毫秒，
  在没有并发写入时不到一次 HTTP 请求往返的量级；而方案 B 的代价每个 PUT 都要付。所以采用方案 A。
- 方案 A 的额外内存：每个快照要多一份索引（key 字符串、版本链的 `Vec` 和 `Arc` 计数），与对象个数成正比，数据本身不复制。

如果以后需要在上百万对象的状态上频繁快照，换成持久化 map 是一处集中的改动（`store::Index` 类型别名，`exp/ordmap` 分支证明了只需要改两处调用）。

## 时钟

服务器的所有时间戳（对象的 `Last-Modified`、版本时间、bucket 创建时间、分段上传的发起时间、条件请求的比较、请求日志的 `time`）
都读自服务器时钟。每个 `Server` 有自己的时钟，同一进程里的多个服务器互不影响。时钟有两种状态：

- **运行**：主机时间加一个偏移量（初始为 0）。
- **冻结**：时间停在一个固定值，直到你推进它。

| 端点 | 请求体 | 作用 |
|---|---|---|
| `GET /_fakes3/clock` | | 读取 |
| `POST /_fakes3/clock/freeze` | `{"at": "2030-01-02T03:04:05Z"}`，`at` 可省略 | 冻结在 `at`，省略则冻结在当前服务器时间 |
| `POST /_fakes3/clock/unfreeze` | | 从当前服务器时间继续运行 |
| `POST /_fakes3/clock/set` | `{"at": "..."}` | 跳到 `at`，保持冻结或运行的状态 |
| `POST /_fakes3/clock/advance` | `{"by": "30d"}` | 前进（`by` 可为负数） |
| `POST /_fakes3/clock/reset` | | 回到主机时钟 |

响应都是当前状态：

```
$ curl -X POST localhost:9000/_fakes3/clock/freeze -d '{"at":"2030-01-02T03:04:05Z"}'
{"now":"2030-01-02T03:04:05.000Z","frozen":true,"shifted":false}
```

`shifted` 表示时钟是否和主机时钟不同（冻结或带偏移）。时钟的取值范围约为 1824～2116 年（内部用一个 63 位纳秒值表示），超出返回 400。
所有状态变化都是原子的：并发的 `advance` 会累加，不会互相覆盖；读取 `now()` 只是一次原子读。

### 预签名 URL 与签名时间

s3s（协议层）**不允许注入时间**：它在校验 SigV4 的 `X-Amz-Date` 时钟偏差（±15 分钟）、预签名 URL 的有效期（`X-Amz-Expires`）、
SigV2 的 `Expires`、POST policy 的 `expiration` 时，直接读取主机时间，没有可替换的接口。我们不 fork s3s，因此：

- **能做到：让预签名 URL 过期。** 时钟被移动过（冻结或有偏移）时，fakes3 在交给 s3s 之前先用服务器时钟检查 URL 的有效期
  （SigV4 的 `X-Amz-Date` + `X-Amz-Expires`，SigV2 的 `Expires`），过期就返回 403 `AccessDenied: Request has expired`。
  所以“生成一个 60 秒有效的 URL，把时钟拨快 2 分钟，再请求”可以得到和 S3 一样的错误。时钟没动时不做这项检查，不影响性能。
- **做不到：** 让主机时钟认为已过期的 URL 重新生效（把时钟拨回去没有用）；让签名日期和主机时间相差超过 15 分钟的请求通过
  （例如客户端自己的时钟被冻结在 2030 年再去签名，会被 s3s 以时钟偏差拒绝）；让 POST policy 的过期时间跟随服务器时钟。
  客户端用主机时间签名、服务器时钟只向前拨，是可用的组合。

对象的 `Last-Modified`、条件请求（`If-Modified-Since` 等）、Object Lock 之类以后要加的到期逻辑，都读服务器时钟，不受这个限制。

## 请求日志

每个 S3 请求（不含 `/_fakes3/` 本身）在响应头就绪后记录一条，放进有界的环形缓冲区（默认 10000 条，满了丢弃最旧的）。
默认开启；`--no-request-log` 启动时关闭，也可以随时通过 API 开关。

请求日志的开销：在同一台 runner 上交替跑三轮 bench，开启与关闭日志（`--no-request-log`）相比，4 KiB PUT（16 和 64 并发）、GET、List 的吞吐差别在
±0.7% 以内（噪声水平），模拟测试套件每请求 246 µs 对 245 µs；开启版本控制的混合负载吞吐低约 1.4%（14,187 对 14,390 ops/s，三轮方向一致）。
和没有控制面的基线相比，开启日志时各项都在 −2.9%（版本控制混合负载）到 +0.8% 之间，其余 −0.7% 以内。

每条记录：

| 字段 | 含义 |
|---|---|
| `seq` | 到达顺序的序号，从 1 开始；`clear` 和 `reset` 不会让它归零 |
| `time` | 请求到达时的服务器时钟时间 |
| `method` | HTTP 方法 |
| `path` | 请求的路径和查询串，原样 |
| `bucket`, `key` | path-style 解析并解码后的 bucket 和 key（没有则为 `null`） |
| `operation` | S3 操作名，如 `PutObject`、`ListObjectsV2` |
| `status` | HTTP 状态码 |
| `latency_us` | 从收到请求到响应头就绪的微秒数（不含响应体的流式发送） |
| `bytes_in` | 请求负载大小：优先 `x-amz-decoded-content-length`，否则 `Content-Length` |
| `bytes_out` | 响应的 `Content-Length`，未知时为 0 |

`operation` 是根据请求的形状（方法、路径、查询参数、是否有 `x-amz-copy-source`）推断的，不是 s3s 报告的：s3s 只在开启签名校验时
才把操作名交给我们，而且签名失败的请求根本到不了那一步。覆盖 fakes3 实现的全部操作和常见的 bucket 子资源；
不认识的请求为空字符串。因为是读取时才推断，记录请求本身没有这部分开销。

### 列出

`GET /_fakes3/requests?...`，按时间从旧到新返回。过滤条件可以组合：

| 参数 | 含义 |
|---|---|
| `bucket` | 精确匹配 bucket |
| `key_prefix` | key 前缀 |
| `op` | 操作名，不区分大小写 |
| `method` | HTTP 方法，不区分大小写 |
| `status` | `404`、`4xx` 或 `error`（400 及以上） |
| `since` | RFC 3339，只要这个时间及之后的请求 |
| `after` | 只要 `seq` 大于它的请求，用于“上次读到哪里” |
| `limit` | 只保留**最近**的 N 条匹配 |

```
$ curl 'localhost:9000/_fakes3/requests?op=PutObject&bucket=logs&limit=1'
{"requests":[{"seq":3,"time":"2026-10-31T07:21:30.998Z","method":"PUT","path":"/logs/b/two.txt","bucket":"logs","key":"b/two.txt",
"operation":"PutObject","status":200,"latency_us":84,"bytes_in":2,"bytes_out":0}],"total":3,"dropped":0}
```

`total` 是过滤后、应用 `limit` 前的匹配数（断言调用次数用它或 `ctl requests list --count`）；`dropped` 是因缓冲区满而被丢弃的记录数。

### 清空与设置

- `DELETE /_fakes3/requests` 清空。
- `GET /_fakes3/requests/config` → `{"enabled":true,"capacity":10000,"stored":42,"dropped":0}`
- `PUT /_fakes3/requests/config`，请求体 `{"enabled": false}`、`{"capacity": 500}` 或两者。缩小容量会丢弃最旧的记录。
  关闭只是停止记录，已有记录保留。

## 统计

`GET /_fakes3/stats`（`fakes3 ctl stats`）：

```
$ curl localhost:9000/_fakes3/stats
{"memory":{"used":314572800,"limit":8589934592},"store":{"buckets":2,"objects":1500,"uploads":1},
 "snapshots":2,"requests":{"enabled":true,"capacity":10000,"stored":42,"dropped":0}}
```

`memory.used` 是服务器持有的对象数据字节数（含分段上传的 part，以及仍被快照或进行中的读取引用的数据），`limit` 是
`--memory-limit` 的实际值，不限制时为 `null`。`GET /_fakes3`（或 `/_fakes3/`）在此之外还返回版本、时钟和 `bucket_defaults`（`"legacy"` 或 `"aws"`，见 [README](../README.md#新建-bucket-的默认值)），`fakes3 ctl status` 显示的就是它。

**内存与重置、快照的关系。** 数据缓冲区按引用计数在最后一个引用消失时释放并归还额度，所以：

- `reset` 之后 `memory.used` 回到 0——除非还有快照引用着那些数据，此时要删除快照（或 `reset?snapshots=true`）才会释放；
- 快照不复制数据，所以创建快照不增加 `used`；但快照之后被覆盖或删除的对象，其数据会因为快照还引用着而继续占用额度，
  在受 `--memory-limit` 限制的服务上，长期保留大量快照可能让后续写入 507。用完的快照请删除；
- `restore` 不分配新的数据缓冲区，只是让存储重新引用快照里的缓冲区；restore 时存储里“只有当前状态才引用”的数据随之释放。

## 限制

- 无认证（见[安全](#安全)）；只能用 `http://`。
- 快照/restore/reset 要在没有未完成请求时调用；快照只存在内存里。
- 时钟不能注入 s3s 的签名校验（见[预签名 URL 与签名时间](#预签名-url-与签名时间)）。
- 操作名由请求形状推断；不认识的为空。
- 请求日志只记 S3 请求；响应体的流式发送时长和错误码不在记录里。
- 内存额度按引用计数释放：快照会让被覆盖或删除的数据继续占用 `--memory-limit` 额度（见[统计](#统计)）。
- 尚未实现故障注入（`/_fakes3/faults`）。

## Rust API

在进程内使用 `fakes3::Server` 时，同样的操作是它的方法，不经过 HTTP：

```rust
let server = fakes3::Server::start(fakes3::Config::default()).await?;

server.snapshot(Some("seeded"))?;                  // -> SnapshotInfo
server.restore("seeded")?;
server.reset();                                    // 数据 + 请求日志 + 时钟，保留快照

server.clock().freeze(None)?;                      // Clock: freeze / unfreeze / set / advance / reset / now
server.clock().advance(fakes3::parse_duration("30d")?)?;

let (requests, total) = server.requests(&fakes3::RequestFilter {
    operation: Some("PutObject".into()),
    ..Default::default()
});

// 其余操作（reset 的选项、快照列表和删除、请求日志开关）在 server.control() 上：
server.control().reset_with(fakes3::ResetOptions { snapshots: true, ..Default::default() });
server.control().delete_snapshot("seeded")?;
server.control().set_request_log_enabled(false);
```
