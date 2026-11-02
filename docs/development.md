# 开发

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

**每次 push 和 PR** 都会自动跑 [ci](../.github/workflows/ci.yml)：格式、clippy（警告即失败）、全部 Rust 测试、
Node binding 的编译，以及 workflow 文件的 actionlint 检查。

比较慢的检查用 `workflow_dispatch` 手动触发：

- **matrix**：先跑单元测试，再对选定的服务跑 SDK 兼容矩阵，报告显示在 job summary 里。
- **s3-tests**：用 ceph/s3-tests 衡量一致性，和 main 逐个用例对比。
- **bench**：对选定的服务跑启动、模拟测试套件、内存占用和 warp 压测。
- **ab**：在同一台 runner 上交替压测两个版本（`base` / `head`），用来判断一个改动对性能的影响。
  托管 runner 之间速度能差 2 倍，所以比较两个版本时只能用这个，不能拿两次独立的 bench 结果比。
- **node**：构建 Node 插件并测试；推送 `v*` tag 时通过 npm 可信发布上传（见 [bindings/node/PUBLISHING.md](../bindings/node/PUBLISHING.md)）。
- **release**：推送 `v*` tag 时发布 GHCR 镜像和静态二进制。

本地也可以跑兼容矩阵和性能测试，前提是装好 Docker 和各语言的工具链：

```bash
bench/setup.sh && uv run bench/run.py      # 性能测试，结果写到 bench/RESULTS.md
SERVERS="fakes3 minio" matrix/run.sh && python3 matrix/report.py   # 兼容矩阵
```

`bench/servers.sh` 负责以相同条件启动所有被测实现，也可以单独用来起一个竞品做对比。

## Node 绑定

```bash
cd bindings/node
npm ci && npm run build   # 需要 Rust 工具链、perl 和 make
npm test                  # 用 @aws-sdk/client-s3 跑一遍
```

CI 在 [node workflow](../.github/workflows/node.yml) 里构建四个目标（x64 / arm64，glibc / musl）并各自跑一遍测试；
发布到 npm 只在推送 `v*` tag 或手动触发且 `publish: true` 时发生，其余情况只做 `npm publish --dry-run`。

## 发布

1. 把根目录 `Cargo.toml` 和 `bindings/node/package.json` 的 `version` 改成新版本，合入 main。
2. 在 main 上推送对应的 tag：`git tag v0.2.0 && git push origin v0.2.0`。
   - [release workflow](../.github/workflows/release.yml) 构建两个架构（各用原生 runner）、冒烟测试，把多架构镜像推到
     `ghcr.io/akarachen/fakes3`（tag：完整版本、`主版本.次版本`、`latest`、短 sha），并把静态二进制挂到 GitHub Release。
   - [node workflow](../.github/workflows/node.yml) 通过 npm 可信发布（OIDC，无 token）上传 `fakes3-sdk` 和四个平台包，自带 provenance。
3. 两个 workflow 都会先检查 tag 和版本号一致。手动触发时 `publish` 默认是 `false`，只构建和测试，不推送任何东西。

npm 可信发布的一次性配置见 [bindings/node/PUBLISHING.md](../bindings/node/PUBLISHING.md)。
