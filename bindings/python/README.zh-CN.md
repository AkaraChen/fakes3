# fakes3-sdk

[English](https://github.com/AkaraChen/fakes3/blob/main/bindings/python/README.md) | 简体中文

[fakes3](https://github.com/AkaraChen/fakes3)（给测试用的内存 S3 服务）的 Python 包和 pytest fixture。
wheel 里带着预编译的 fakes3 二进制：不需要 Docker，运行时也不下载任何东西。一个服务几毫秒就能起来，所以每个测试都可以有自己的一个。

```bash
pip install fakes3-sdk
```

导入名是 `fakes3`。包本身没有任何依赖；示例里用的是 boto3。

## pytest

装上包就注册好了 fixture。`fakes3_server` 给每个测试一个全新的空服务，测试结束时停掉，不管测试是通过、失败还是被中断：

```python
import urllib.error
import urllib.request

import pytest


def test_upload(fakes3_server):
    s3 = fakes3_server.boto3_client()  # 指向这个服务的 boto3 客户端，path-style
    s3.create_bucket(Bucket="demo")
    s3.put_object(Bucket="demo", Key="hello.txt", Body=b"hi")

    assert fakes3_server.requests.count(operation="PutObject") == 1


def test_link_expires(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="share")
    s3.put_object(Bucket="share", Key="report.csv", Body=b"...")
    url = s3.generate_presigned_url(
        "get_object", Params={"Bucket": "share", "Key": "report.csv"}, ExpiresIn=60
    )

    fakes3_server.clock.advance("2m")  # 拨的是服务的时钟，不是你机器的
    with pytest.raises(urllib.error.HTTPError) as e:
        urllib.request.urlopen(url)
    assert e.value.code == 403  # AccessDenied: Request has expired
```

如果要把连接信息交给你自己的代码，用 `fakes3_server.endpoint`、`fakes3_server.access_key` 和
`fakes3_server.secret_key`（除非 `auth=True`，否则任何凭证都能用）。

| Fixture | 作用域 | |
| --- | --- | --- |
| `fakes3_server` | function | 每个测试一个新服务 |
| `fakes3_session_server` | session | 整个测试运行共用一个（pytest-xdist 下每个 worker 一个）。测试之间共享数据，除非调用 `reset()` 或 `restore()` |
| `fakes3_options` | session | 上面两个传给 `fakes3.start()` 的关键字参数，在 `conftest.py` 里覆盖它 |

```python
# conftest.py
import pytest

@pytest.fixture(scope="session")
def fakes3_options():
    return {"bucket_defaults": "aws", "memory_limit": "1GiB"}
```

测试数据很多、只想灌一次时，也可以共用一个服务，每个测试前回滚：

```python
@pytest.fixture
def s3_store(fakes3_session_server):
    fakes3_session_server.restore("seeded")  # 灌完数据后存的快照
    yield fakes3_session_server
```

## 不用 pytest

```python
import fakes3

with fakes3.start() as server:      # 127.0.0.1 的空闲端口
    s3 = server.boto3_client()
    ...
# 到这里已经停了

server = fakes3.start()
...
server.stop()                        # 可以重复调用
```

`start()` 在服务能接受请求后才返回。服务是一个子进程，不会比你的进程活得久：`stop()`、离开 `with` 块、对象被垃圾回收、
解释器退出都会停掉它；如果你的进程被直接杀掉，服务会发现 stdin 管道关闭，自己退出。

### 选项

都是 `start()` 的关键字参数，都可以省略：

| 选项 | 默认值 | 说明 |
| --- | --- | --- |
| `host` | `"127.0.0.1"` | 监听地址 |
| `port` | `0` | 监听端口，`0` 表示随机空闲端口 |
| `access_key` / `secret_key` | `"fakes3"` / `"fakes3secret"` | 默认账户的凭证，只在 `auth=True` 时校验 |
| `auth` | `False` | 校验签名。默认任何凭证都能用：签名请求以其 access key 所属的账户身份执行（不属于任何账户时用默认账户），未签名的请求是匿名请求，照常受 ACL、Bucket Policy 和 Block Public Access 约束 |
| `users` | 无 | 更多账户：字典，含 `name`、`access_key`、`secret_key`，可选 `account_id`、`canonical_id` |
| `bucket_defaults` | `"legacy"` | 新建 bucket 的初始设置：`"legacy"`，或 `"aws"`（AWS 现在的默认：ACL 关闭、Block Public Access 打开、SSE-S3、禁止 SSE-C） |
| `memory_limit` | `"auto"` | 对象数据的上限：字节数、`"512MiB"`、`"2GiB"`、`"unlimited"`，或 `"auto"`（容器或机器内存的 75%）。超过后写入返回 HTTP 507 |
| `domains` | `["localhost"]` | virtual-host 寻址的基础域名（`<bucket>.<domain>`）；path-style 总是可用 |
| `inline_threshold` | `1048576` | Linux 上达到这个字节数的对象存进 `memfd`，而不是堆上 |
| `request_log` / `request_log_capacity` | `True` / `10000` | 是否记录请求供 `server.requests` 查询，以及最多保留多少条 |
| `args` | 无 | 额外传给 `fakes3` 二进制的命令行参数 |
| `startup_timeout` | `10` | 等待服务启动的秒数 |

环境变量里的 `FAKES3_*` 会被忽略，服务只按这些选项配置。

## `FakeS3`

| 成员 | 说明 |
| --- | --- |
| `endpoint`、`host`、`port` | 监听的地址。`endpoint` 是 `http://host:port`，可以直接当 `endpoint_url` |
| `access_key`、`secret_key`、`region` | 默认账户的凭证，以及 `"us-east-1"` |
| `boto3_client(**overrides)`、`boto3_kwargs()` | 指向这个服务的 boto3 S3 客户端（path-style），或者构造它的参数。需要装 boto3 |
| `stop()`、`running`、`pid` | 停止服务；是否在运行；进程号 |
| `reset(clock=True, requests=True, snapshots=False)` | 清空所有 bucket、对象和分段上传，默认同时清空请求日志、恢复时钟。快照保留，除非 `snapshots=True` |
| `snapshot(name=None)`、`restore(name)`、`snapshots()`、`delete_snapshot(name)` | 保存当前状态并回滚到它，次数不限。对象数据共享，不复制 |
| `memory()`、`stats()`、`status()` | 持有的对象数据（`{"used", "limit"}`）、数据量，以及服务的配置 |
| `clock` | 服务的时钟（见下） |
| `requests` | 请求日志（见下） |

服务拒绝的控制调用会抛出 `fakes3.FakeS3Error`，带服务返回的 `code`（`"NotFound"`、`"Conflict"` 等）和 HTTP `status`。
`stop()` 之后再调用会抛出 `server is stopped`。

### 虚拟时钟

服务产生的所有时间戳都读这个时钟，所有和时间有关的行为也是：预签名 URL 的过期、Lifecycle 规则、Object Lock 保留期、归档对象的恢复。

```python
from datetime import datetime, timedelta, timezone

server.clock.freeze(datetime(2030, 1, 2, tzinfo=timezone.utc))  # 不带参数则冻结在当前时间
server.clock.advance("31d")              # 也可以是 "1h30m"、"-2w"、timedelta，或秒数
server.clock.set(at)                     # 跳到某个时间，保持冻结或运行的状态
server.clock.unfreeze()                  # 让时间继续走
server.clock.reset()                     # 回到主机时钟
server.clock.now()                       # UTC 的 aware datetime；还有 is_frozen()、is_shifted()
```

不带时区的 datetime 按 UTC 处理。客户端用主机时间签名，所以服务的时钟只往前拨：主机时钟认为已经过期的预签名 URL，拨回去也不会重新生效。

### 请求日志

断言你的代码实际发出了哪些请求：

```python
server.requests.list(operation="PutObject", status="error", limit=10)
server.requests.count(bucket="demo")        # 匹配的条数，不受 limit 影响
server.requests.clear()
server.requests.configure(enabled=True, capacity=50_000)
```

记录是字典，按到达顺序：
`{seq, time, method, path, bucket, key, operation, status, latency_us, bytes_in, bytes_out, access_key, request_id}`。
过滤条件：`bucket`、`key_prefix`、`operation`、`method`、`status`（`404`、`"4xx"` 或 `"error"`）、`since`、
`after`（一个 `seq`）和 `limit`（只保留最近的 N 条匹配）。

控制调用和正在进行的请求之间没有同步，请在测试之间、没有未完成请求时调用。

## 命令行

包里也装了 `fakes3` 命令，和 release 里的二进制是同一个：

```bash
fakes3 --port 0          # 开始监听后打印 endpoint
fakes3 ctl --endpoint http://127.0.0.1:9000 clock advance 1d
```

## 平台

| Wheel | 平台 |
| --- | --- |
| `manylinux_2_17_x86_64`、`musllinux_1_1_x86_64` | Linux x86_64（静态二进制：任何 glibc 或 musl，包括 Alpine） |
| `manylinux_2_17_aarch64`、`musllinux_1_1_aarch64` | Linux aarch64（静态） |
| `macosx_11_0_arm64` | Apple silicon 的 macOS |
| `macosx_10_12_x86_64` | Intel 的 macOS |
| `win_amd64` | Windows x64 |

需要 Python 3.10 或更新版本。其他平台上，把 `FAKES3_BINARY` 设为你自己构建的 fakes3 二进制（`cargo install fakes3`）。

## 安全提示

fakes3 是给测试用的。默认不校验签名，控制面也没有认证，所以不要把服务暴露到不可信的网络，也不要往里存真实数据。
除非传了 `host`，它只监听 `127.0.0.1`。

## 从源码构建

```bash
cd bindings/python
pip install .                         # 会执行 cargo build --release；需要 Rust、perl 和 make
pip install pytest boto3 && pytest
```

`python build_backend.py --binary path/to/fakes3 --platform <tag>` 把一个已有的二进制打成 wheel。

## 许可证

MIT
