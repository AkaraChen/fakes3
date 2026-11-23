# fakes3 的 Go 模块

[English](README.md) | 简体中文

在 Go 测试里启动 [fakes3](https://github.com/AkaraChen/fakes3)：一个全内存的 S3 服务。不需要 Docker：
每个服务器是一个小的子进程，几毫秒就绪，所以每个测试都可以有自己的一个。

```bash
go get github.com/AkaraChen/fakes3/bindings/go
```

```go
import (
	"testing"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"

	fakes3 "github.com/AkaraChen/fakes3/bindings/go"
)

func TestUpload(t *testing.T) {
	t.Parallel()
	srv := fakes3.Start(t) // 一个新的服务器，随机空闲端口，测试结束时自动停止
	client := s3.New(s3.Options{
		BaseEndpoint: aws.String(srv.Endpoint),
		Region:       srv.Region,
		UsePathStyle: true,
		Credentials:  credentials.NewStaticCredentialsProvider(srv.AccessKey, srv.SecretKey, ""),
	})
	// ……用 client 调你的代码，再检查 bucket 里的内容
}
```

每个服务器有独立的数据、时钟和端口，`t.Parallel()` 并行的测试看不到彼此的 bucket。模块本身只用标准库，
任何 S3 客户端都可以（例子里用的是 aws-sdk-go-v2）。需要 Go 1.24 或更高版本。

## 二进制从哪里来

按顺序取第一个：

1. `Options.Binary`
2. 环境变量 `FAKES3_BINARY`
3. `PATH` 里的 `fakes3`
4. 从 [GitHub Releases](https://github.com/AkaraChen/fakes3/releases) 下载这个模块对应的 fakes3 版本（`fakes3.Version`）

每个版本只下载一次：本平台的压缩包要和 release 附带的 SHA-256 校验值一致，解出的二进制放在用户缓存目录
（Linux 是 `~/.cache/fakes3`，macOS 是 `~/Library/Caches/fakes3`，Windows 是 `%LocalAppData%\fakes3`）。
校验值对不上时 `Start` 让测试失败，什么也不缓存。

| 环境变量 | 选项 | 说明 |
| --- | --- | --- |
| `FAKES3_BINARY` | `Binary` | 要运行的 fakes3 可执行文件路径 |
| `FAKES3_VERSION` | `Version` | 下载这个版本而不是 `fakes3.Version`，如 `0.1.4` |
| `FAKES3_CACHE_DIR` | `CacheDir` | 下载的二进制放在哪里 |
| `FAKES3_DOWNLOAD_URL` | `DownloadURL` | 下载 release 的基础 URL，用于镜像；文件地址是 `<url>/v<版本>/<压缩包>` |

有预编译二进制的平台：Linux x86_64 和 arm64、Apple silicon 和 Intel 的 macOS、Windows x86_64。
其他平台用 `cargo install fakes3 --locked` 自己构建，再把 `FAKES3_BINARY` 指向它。

在 CI 里可以缓存这个目录省掉下载，或者事先把 fakes3 装到 `PATH` 上（比如用 [`setup-fakes3`](https://github.com/AkaraChen/setup-fakes3)）。

## 启动服务器

| 函数 | 说明 |
| --- | --- |
| `Start(t)` | 用默认选项启动。启动失败时让测试失败；在 `t.Cleanup` 里停止服务器 |
| `StartWith(t, opts)` | 同上，带 `Options` |
| `New(ctx, opts)` | 返回 `(*Server, error)`，给测试之外的地方用，比如 `TestMain`。需要自己调用 `Close` |

`Options` 的字段都可以不填：

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `Host` | `"127.0.0.1"` | 监听地址 |
| `AccessKey` / `SecretKey` | `"fakes3"` / `"fakes3secret"` | 默认账户的凭证，只在 `Auth` 时校验 |
| `Auth` | `false` | 校验签名。默认任何凭证都能用：带签名的请求算作它的 access key 所属的账户（不属于任何账户时算默认账户），不带签名的请求是匿名请求，照常受 ACL、Bucket Policy 和 Block Public Access 约束 |
| `Users` | 无 | 更多账户：`[]fakes3.User{{Name, AccessKey, SecretKey, AccountID, CanonicalID}}`，两个 id 可以不填 |
| `BucketDefaults` | `"legacy"` | 新建 bucket 的初始设置：`"legacy"`，或者 `"aws"`，即 AWS 现在的默认值（禁用 ACL、开启 Block Public Access、SSE-S3、禁止 SSE-C） |
| `MemoryLimit` | `"auto"` | 对象数据的上限：`"512MiB"`、`"2GiB"`、`"unlimited"` 或 `"auto"`（容器或机器内存的 75%）。超出的写入返回 HTTP 507 |
| `Domains` | `["localhost"]` | virtual-host 寻址的基础域名（`<bucket>.<domain>`）；path-style 始终可用 |
| `DisableRequestLog` / `RequestLogCapacity` | `false` / `10000` | 关闭请求日志，或者修改它保留的条数 |
| `Args` | 无 | 额外的 fakes3 命令行参数 |
| `Stderr` | 无 | 服务器日志写到哪里。不设置时，日志只在启动失败时出现在错误信息里 |
| `StartTimeout` | 30 秒 | 启动最多等多久，不含下载 |

配置 fakes3 服务本身的 `FAKES3_*` 环境变量（比如 `FAKES3_AUTH`）不会传给子进程，所以 shell 里的设置不会改变测试服务器的行为；请用 `Options`。

## `Server`

| 成员 | 说明 |
| --- | --- |
| `Endpoint`、`Host`、`Port` | 监听的地址。`Endpoint` 是 `http://host:port`，直接给 path-style 的 S3 客户端用 |
| `AccessKey`、`SecretKey`、`Region` | 配置客户端用的凭证和 region（`us-east-1`） |
| `Close()` | 停止服务器，返回时端口已经释放。可以重复调用 |
| `Reset()`、`ResetWith(fakes3.ResetOptions{...})` | 删除所有 bucket、对象和分段上传，清空请求日志，恢复时钟。快照默认保留，`Snapshots: true` 时一起删除；`KeepClock`、`KeepRequests` 保留时钟或请求日志 |
| `Snapshot(name)`、`Restore(name)`、`Snapshots()`、`DeleteSnapshot(name)` | 保存当前状态并回滚到它，可以反复回滚。对象数据共享、不复制。名字为空时自动生成 |
| `Memory()` | 持有的对象数据字节数和上限（不限制时为 0） |
| `Clock()` | 服务器时钟（见下文） |
| `Requests()` | 请求日志（见下文） |

控制方法返回 error；服务器拒绝时（比如 `Restore` 一个不存在的快照）是带 HTTP 状态码和错误码的 `*fakes3.Error`。
它们和正在进行的 S3 请求没有同步，请在没有未完成请求时调用。

### 虚拟时钟

服务器产生的所有时间戳都读这个时钟，依赖时间的功能也一样：预签名 URL 的过期、Lifecycle 规则、Object Lock 保留期、归档对象的恢复。

```go
srv.Clock().FreezeAt(time.Date(2030, 1, 2, 3, 4, 5, 0, time.UTC)) // Freeze() 冻结在当前时间
srv.Clock().Advance(31 * 24 * time.Hour) // 负数表示后退
srv.Clock().Set(t)                       // 跳到 t，保持冻结或运行的状态
srv.Clock().Unfreeze()                   // 让时间继续走
srv.Clock().Reset()                      // 回到主机时钟
srv.Clock().Now()                        // time.Time；State() 还能看出是否冻结、是否偏移
```

时钟走过预签名 URL 的有效期后，URL 就过期了，返回和 S3 一样的 403：

```go
req, _ := s3.NewPresignClient(client).PresignGetObject(ctx, &s3.GetObjectInput{
	Bucket: aws.String("reports"), Key: aws.String("q3.csv"),
}, s3.WithPresignExpires(time.Minute))

srv.Clock().Advance(2 * time.Minute)
resp, _ := http.Get(req.URL) // 403 AccessDenied: Request has expired
```

把时钟往回拨不能让按主机时间已经过期的 URL 重新生效；客户端要用主机时间签名（AWS SDK 就是这样），因为和主机时间相差超过 15 分钟的签名会被拒绝。

### 请求日志

断言你的代码实际发了哪些请求：

```go
puts, err := srv.Requests().List(fakes3.RequestFilter{Bucket: "reports", Operation: "PutObject"})
n, err := srv.Requests().Count(fakes3.RequestFilter{Status: "error"}) // 忽略 Limit
srv.Requests().Clear()
srv.Requests().SetEnabled(false) // 还有 SetCapacity(n) 和 Config()
```

每条 `fakes3.Request` 有 `Seq`、`Time`、`Method`、`Path`、`Bucket`、`Key`、`Operation`、`Status`、`LatencyUs`、`BytesIn`、
`BytesOut`、`AccessKey` 和 `RequestID`，按到达顺序排列。过滤字段：`Bucket`、`KeyPrefix`、`Operation`、`Method`、
`Status`（`"404"`、`"4xx"` 或 `"error"`）、`Since`、`After`（一个 `Seq`）和 `Limit`（只保留最近的几条）。

## 整个包共用一个服务器

每个测试起一个服务器很便宜，但也可以共用一个，在测试之间把它恢复到已知状态：

```go
var srv *fakes3.Server

func TestMain(m *testing.M) {
	var err error
	srv, err = fakes3.New(context.Background(), fakes3.Options{})
	if err != nil {
		log.Fatal(err)
	}
	// 在这里建好 bucket 和对象，然后：srv.Snapshot("seeded")
	code := m.Run()
	srv.Close()
	os.Exit(code)
}

func TestSomething(t *testing.T) {
	srv.Restore("seeded") // 或者 srv.Reset()
	// ...
}
```

共用服务器的测试不能用 `t.Parallel()`。

## 停止

`Start` 在测试结束时停止服务器，测试失败或 panic 时也一样。如果测试进程没来得及执行 cleanup 就退出了
（其他 goroutine 里的 panic、`os.Exit`、`go test -timeout` 超时或者被杀掉），服务器也会跟着退出：Linux 上父进程退出时由内核杀掉它，
macOS 上由一个小的看门狗进程杀掉，Windows 上它运行在一个 job object 里，随测试进程一起关闭。

## 版本

`fakes3.Version` 是这个模块版本下载的 fakes3 版本。模块的 tag 是 `bindings/go/vX.Y.Z`，跟随它固定的 fakes3 版本；
`go get github.com/AkaraChen/fakes3/bindings/go@latest` 取最新版。

## 安全提示

fakes3 是给测试用的。默认不校验签名，控制接口也没有认证，所以不要把它暴露到不可信的网络，也不要存放真实数据。
除非设置 `Host`，它只监听 `127.0.0.1`。

## 许可证

MIT
