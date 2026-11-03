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
- **release**：推送 `v*` tag 时发布 GHCR 镜像、静态二进制和 crates.io 上的 `fakes3` crate；其余情况只打包并校验 crate（`cargo publish --dry-run`）。

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
     构建通过后，`crate` job 通过 crates.io 可信发布（OIDC，无 token）上传 crate；这个版本已经在 crates.io 上时跳过上传，
     所以重跑整个 workflow 不会失败。
   - [node workflow](../.github/workflows/node.yml) 通过 npm 可信发布（OIDC，无 token）上传 `fakes3-sdk` 和四个平台包，自带 provenance。
3. 两个 workflow 都会先检查 tag 和版本号一致。手动触发时 `publish` 默认是 `false`，只构建和测试，不推送任何东西。

npm 可信发布的一次性配置见 [bindings/node/PUBLISHING.md](../bindings/node/PUBLISHING.md)。

### crates.io 可信发布的一次性配置

crates.io 只能给已经存在的 crate 添加可信发布者，所以第一个版本要由人用 API token 手动发布：

1. 在 crates.io 用 GitHub 登录，到 Account Settings → API Tokens 新建一个 token：scope 只选 `publish-new`，
   crate 填 `fakes3`，过期时间设短一些。
2. 在最新的 main 上执行 `cargo login`（粘贴 token），再执行 `cargo publish --locked`。发布前会从打包后的源码重新编译一遍，
   其中包括 vendored OpenSSL，所以需要 perl 和 make。
3. 打开 crates.io 上 `fakes3` 的 Settings → Trusted Publishing → Add → GitHub：Repository owner 填 `AkaraChen`，
   Repository name 填 `fakes3`，Workflow filename 填 `release.yml`，Environment 留空。
4. 在同一页勾选 “Require trusted publishing for all new versions”，之后用 API token 发布会被拒绝。
5. `cargo logout`，并在网页上撤销第 1 步的 token。

之后每次推送 `v*` tag，release workflow 都会自动发布对应版本。
