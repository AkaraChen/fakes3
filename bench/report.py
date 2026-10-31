"""Render a bench/results/*.json file into bench/RESULTS.md.

    python3 bench/report.py [results.json]     # default: newest file in bench/results/
"""

import json
import statistics
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ORDER = ["fakes3", "minio", "localstack", "moto", "s3mock", "juicefs"]

WORKLOADS = [
    ("small_get", "GET", "4 KiB GET，16 并发"),
    ("small_get_c1", "GET", "4 KiB GET，单并发（纯延迟）"),
    ("small_put", "PUT", "4 KiB PUT，16 并发"),
    ("small_put_c64", "PUT", "4 KiB PUT，64 并发（同一 bucket 上的写入争用）"),
    ("stat", "STAT", "HEAD，16 并发"),
    ("list", "LIST", "ListObjects，8 并发"),
    ("delete", "DELETE", "DeleteObjects 批量删除（每批 100），8 并发"),
    ("versioned", None, "开启版本控制的混合负载 4 KiB（GET / PUT / DELETE / STAT），16 并发"),
    ("mixed", None, "混合负载 64 KiB（GET 45% / STAT 30% / PUT 15% / DELETE 10%），16 并发"),
    ("large_put", "PUT", "64 MiB 单次 PUT，4 并发（只跑 5 秒，避免写满内存）"),
    ("large_get", "GET", "64 MiB GET，4 并发"),
    ("multipart", "GET", "按 part 读取分段对象（GET ?partNumber=，每个 part 5 MiB），8 并发"),
]


def med(xs):
    return statistics.median(xs) if xs else None


def f(x, nd=1, unit=""):
    if x is None:
        return "-"
    if isinstance(x, float) and x >= 100:
        nd = 0
    return f"{x:,.{nd}f}{unit}"


def ratio(base, x, higher_better=True):
    if not base or not x:
        return ""
    r = base / x if higher_better else x / base
    return f"{r:,.1f}×"


def main():
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else max((HERE / "results").glob("*.json"))
    d = json.loads(path.read_text())
    S = d["servers"]
    servers = [s for s in ORDER if s in S]
    out = ["# 性能对比", ""]
    h = d["host"]
    out += [
        f"由 `bench/run.py` 生成，原始数据：`{path.relative_to(HERE.parent) if path.is_relative_to(HERE.parent) else path}`。",
        "",
        f"环境：{h['cpu']}，{h['cores']} 核，{h['mem_gib']} GiB 内存，Linux {h['kernel']}，测试时间 {d['date']}。",
        "",
        "所有实现都跑在 Docker 里：host 网络、数据目录放在容器内的 tmpfs、使用相同的凭证；每项测量都用一个新启动的容器。",
        "内存取自容器 cgroup（`memory.current` / `memory.peak`，包含 tmpfs 页），CPU 取自 cgroup 的 `cpu.stat`。",
        "fakes3 使用仓库里 Dockerfile 构建的镜像（Alpine + 静态链接的 musl 二进制）。"
        "竞品版本：MinIO 用 Chainguard 从源码构建的最新版（官方镜像已下架）；LocalStack 4.14（最后一个免登录的社区版）；"
        "moto、S3Mock、JuiceFS 用当时的最新镜像（JuiceFS gateway 的元数据用 SQLite，数据用本地文件，两者都在 tmpfs 上）。",
        "",
        "「倍数」列表示 fakes3 相对该实现的倍数：吞吐类为 fakes3 ÷ 对方，延迟、CPU、内存类为对方 ÷ fakes3。数值越大，fakes3 越占优。",
        "",
    ]

    # Startup
    if any("startup_sec" in S[s] for s in servers):
        n = len(S[servers[0]].get("startup_sec", []))
        out += ["## 启动", ""]
        out += [
            "「总计」是从执行 `docker run` 到第一个带签名的 ListBuckets 返回 200 的时间。"
            "「容器启动后」是 `docker run -d` 返回（容器进程已经启动）到就绪的时间，去掉了每个实现都要付出的 Docker 开销，"
            f"更接近服务本身的启动耗时。每个服务测 {n} 次，取中位数。"
            "就绪探测每 1 ms 轮询一次，低于这个粒度的数值只说明「几乎瞬间就绪」，对应的倍数仅供参考。",
            "",
            "| 实现 | 总计 (ms) | 容器启动后 (ms) | 倍数 | 空闲内存 (MiB) | 倍数 |",
            "|---|---:|---:|---:|---:|---:|",
        ]
        net = lambda r: med([a - b for a, b in zip(r["startup_sec"], r["docker_run_sec"])])
        fs = S.get("fakes3", {})
        fs_net = net(fs) if fs.get("startup_sec") else None
        for s in servers:
            r = S[s]
            if "startup_sec" not in r:
                continue
            t, n_ = med(r["startup_sec"]), net(r)
            out.append(
                f"| {s} | {f(t * 1000, 0)} | {f(n_ * 1000, 0)} | {ratio(fs_net, n_, False) if s != 'fakes3' else ''} "
                f"| {f(r.get('mem_idle_mib'))} | {ratio(fs.get('mem_idle_mib'), r.get('mem_idle_mib'), False) if s != 'fakes3' else ''} |"
            )
        nat = d.get("native_startup_sec", {})
        if nat:
            out += ["", "不经过 Docker、直接启动进程（数据目录放在 /dev/shm），从 spawn 到第一个 ListBuckets 返回 200：", "", "| 实现 | 就绪 (ms) | 倍数 |", "|---|---:|---:|"]
            fn = med(nat.get("fakes3", []))
            for k, v in nat.items():
                out.append(f"| {k} | {f(med(v) * 1000, 1)} | {ratio(fn, med(v), False) if k != 'fakes3' else ''} |")
        out.append("")

    # Suite
    if any("suite" in S[s] for s in servers):
        out += [
            "## 模拟测试套件",
            "",
            "`bench/suite`（aws-sdk-go-v2）顺序执行 100 个「测试」。每个测试依次：建 bucket → PUT 20 个 1 KiB 对象 → List → 逐个 GET + HEAD → Copy → 批量删除 → 删 bucket，"
            "共 6,500 个请求。请求是串行的，所以结果几乎完全由单请求延迟决定，这正是测试套件跑得快不快的关键。",
            "",
            "| 实现 | 总耗时 (ms) | 每请求 (µs) | 倍数 | 单个测试 p50 / p99 (ms) | 服务端 CPU (s) | 倍数 | 峰值内存 (MiB) |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
        fs = S.get("fakes3", {}).get("suite", {})
        for s in servers:
            r = S[s].get("suite")
            if not r:
                continue
            if "error" in r:
                out.append(f"| {s} | 失败：{r['error'][:80]} | | | | | | |")
                continue
            out.append(
                f"| {s} | {f(r['total_ms'], 0)} | {f(r['per_request_us'], 0)} | {ratio(fs.get('per_request_us'), r['per_request_us'], False) if s != 'fakes3' else ''} "
                f"| {f(r['per_test_p50'])} / {f(r['per_test_p99'])} | {f(r['server_cpu_sec'], 2)} | {ratio(fs.get('server_cpu_sec'), r['server_cpu_sec'], False) if s != 'fakes3' else ''} | {f(r['mem_peak_mib'])} |"
            )
        out.append("")

    # Footprint
    if any("footprint" in S[s] for s in servers):
        out += [
            "## 内存占用（固定数据集）",
            "",
            "写入固定的数据集：10,000 个 4 KiB 对象加 8 个 64 MiB 对象，逻辑数据量共 551 MiB，16 并发。"
            "「写入后」是写完时容器的内存占用（包括 tmpfs），「额外开销」是它减去逻辑数据量的部分。",
            "",
            "| 实现 | 写入耗时 (s) | 倍数 | 写入后 (MiB) | 额外开销 (MiB) | 峰值 (MiB) | 服务端 CPU (s) |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
        fs = S.get("fakes3", {}).get("footprint", {})
        for s in servers:
            r = S[s].get("footprint")
            if not r:
                continue
            if "error" in r:
                out.append(f"| {s} | 失败：{r['error'][:80]} | | | | | |")
                continue
            logical = r["logical_bytes"] / (1 << 20)
            out.append(
                f"| {s} | {f(r['load_ms'] / 1000, 2)} | {ratio(fs.get('load_ms'), r['load_ms'], False) if s != 'fakes3' else ''} | {f(r['mem_end_mib'])} "
                f"| {f(r['mem_end_mib'] - logical)} | {f(r['mem_peak_mib'])} | {f(r['server_cpu_sec'], 2)} |"
            )
        out.append("")

    # warp
    out += [
        "## 吞吐与延迟（MinIO warp）",
        "",
        "使用 MinIO 官方的 S3 压测工具 [warp](https://github.com/minio/warp)，每项跑固定时长。延迟取 warp 记录的单请求耗时；"
        f"「平均 CPU」是整个过程（包括 warp 的数据准备阶段）中服务端容器平均占用的 CPU 核数。机器共 {h['cores']} 核，warp 客户端也要占用一部分。",
        "",
    ]
    for key, op, title in WORKLOADS:
        if not any(key in S[s] for s in servers):
            continue
        out += [f"### {title}", "", "| 实现 | ops/s | MiB/s | 倍数 | p50 (ms) | p99 (ms) | 平均 CPU (核) | 峰值内存 (MiB) | 错误 |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
        base = None
        rows = []
        for s in servers:
            r = S[s].get(key)
            if not r:
                continue
            if "error" in r:
                rows.append(f"| {s} | 失败：{r['error'][-100:].strip()} | | | | | | | |")
                continue
            ops = r["ops"]
            if op:
                o = ops.get(op, {})
            else:  # mixed: sum across op types
                o = {
                    "ops_per_sec": sum(x["ops_per_sec"] for x in ops.values()),
                    "mib_per_sec": sum(x["mib_per_sec"] for x in ops.values()),
                    "errors": sum(x["errors"] for x in ops.values()),
                }
            tput = o.get("ops_per_sec")
            if s == "fakes3":
                base = tput
            cores = r["server_cpu_sec"] / r["wall_sec"] if r.get("wall_sec") else None
            rows.append(
                f"| {s} | {f(tput)} | {f(o.get('mib_per_sec'))} | {ratio(base, tput) if s != 'fakes3' else ''} | {f(o.get('p50_ms'), 2)} | {f(o.get('p99_ms'), 2)} "
                f"| {f(cores, 2)} | {f(r['mem_peak_mib'])} | {o.get('errors', 0)} |"
            )
        out += rows + [""]

    (HERE / "RESULTS.md").write_text("\n".join(out) + "\n")
    print(HERE / "RESULTS.md")


if __name__ == "__main__":
    main()
