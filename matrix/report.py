"""Render matrix/results/<server>/<sdk>.json into matrix/MATRIX.md."""

import json
import re
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER_ORDER = ["fakes3", "minio", "localstack", "moto", "s3mock", "juicefs"]
ICON = {"pass": "✅", "fail": "❌", "skip": "➖", None: "·"}


def scenario_order():
    ids = re.findall(r"^\| `([a-z0-9_]+)` \|", (HERE / "SPEC.md").read_text(), re.M)
    return list(dict.fromkeys(ids))


def load():
    # data[server][sdk] = {"lang":…, "version":…, "results": {scenario: {...}}}
    data = defaultdict(dict)
    for f in sorted((HERE / "results").glob("*/*.json")):
        try:
            d = json.loads(f.read_text())
        except json.JSONDecodeError:
            continue
        data[f.parent.name][d["sdk"]] = {
            "lang": d.get("lang", "?"),
            "version": d.get("version", "?"),
            "results": {r["scenario"]: r for r in d.get("results", [])},
        }
    return data


def esc(s):
    return str(s).replace("|", "\\|").replace("\n", " ")


def main():
    data = load()
    servers = [s for s in SERVER_ORDER if s in data] + sorted(set(data) - set(SERVER_ORDER))
    scenarios = scenario_order()
    sdks = sorted({(v["lang"], sdk, v["version"]) for srv in data.values() for sdk, v in srv.items()})

    out = ["# SDK 兼容矩阵", "", "由 `matrix/report.py` 根据 `matrix/results/` 生成，请勿手工编辑。场景定义见 [SPEC.md](SPEC.md)。", ""]
    out += ["图例：✅ 通过　❌ 失败　➖ SDK 不支持该能力（skip）　· 未运行", ""]

    # Summary: pass rate per server over the (sdk, scenario) cells that are not skipped.
    out += ["## 汇总", "", "通过率 = 通过 / (通过 + 失败)，不计 skip。", ""]
    out.append("| Server | 通过 | 失败 | skip | 通过率 |")
    out.append("|---|---:|---:|---:|---:|")
    for s in servers:
        c = defaultdict(int)
        for v in data[s].values():
            for r in v["results"].values():
                c[r["status"]] += 1
        tot = c["pass"] + c["fail"]
        rate = f"{100 * c['pass'] / tot:.1f}%" if tot else "-"
        out.append(f"| {s} | {c['pass']} | {c['fail']} | {c['skip']} | {rate} |")
    out.append("")

    # Per server × SDK pass counts.
    out += ["## 每个 SDK 的通过数", ""]
    out.append("| 语言 | SDK | 版本 | " + " | ".join(servers) + " |")
    out.append("|---|---|---|" + "---:|" * len(servers))
    for lang, sdk, ver in sdks:
        cells = []
        for s in servers:
            v = data[s].get(sdk)
            if not v:
                cells.append("·")
                continue
            rs = v["results"].values()
            p = sum(r["status"] == "pass" for r in rs)
            f = sum(r["status"] == "fail" for r in rs)
            cells.append(f"{p}/{p + f}")
        out.append(f"| {lang} | {sdk} | {esc(ver)} | " + " | ".join(cells) + " |")
    out.append("")

    # Full grid per server.
    for s in servers:
        cols = [sdk for _, sdk, _ in sdks if sdk in data[s]]
        out += [f"## {s}", "", "| 场景 | " + " | ".join(cols) + " |", "|---|" + ":-:|" * len(cols)]
        for sc in scenarios:
            row = [ICON[data[s][sdk]["results"].get(sc, {}).get("status")] for sdk in cols]
            out.append(f"| `{sc}` | " + " | ".join(row) + " |")
        out.append("")
        fails = [
            (sc, sdk, data[s][sdk]["results"][sc].get("error", ""))
            for sc in scenarios
            for sdk in cols
            if data[s][sdk]["results"].get(sc, {}).get("status") == "fail"
        ]
        if fails:
            out += ["<details><summary>失败详情</summary>", "", "| 场景 | SDK | 错误 |", "|---|---|---|"]
            out += [f"| `{sc}` | {sdk} | {esc(e)[:200]} |" for sc, sdk, e in fails]
            out += ["", "</details>", ""]

    (HERE / "MATRIX.md").write_text("\n".join(out) + "\n")
    print(HERE / "MATRIX.md")


if __name__ == "__main__":
    main()
