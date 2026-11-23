# 发布 fakes3-sdk 到 PyPI

PyPI 上的包名是 `fakes3-sdk`（导入名 `fakes3`），和 npm 包同名。`fakes3` 这个名字在 PyPI 上没人用，但 PyPI 会拒绝与已有项目
“过于相似”的新名字（去掉 `-`、`_`、`.` 后比较），而 [`fake-s3`](https://pypi.org/project/fake-s3/) 已经存在，所以用不了。

[python workflow](../../.github/workflows/python.yml) 构建 5 个 wheel，每个带一个平台的 fakes3 二进制：

| Wheel 平台标签 | 二进制 | 构建方式 |
| --- | --- | --- |
| `manylinux_2_17_x86_64.manylinux2014_x86_64.musllinux_1_1_x86_64` | x86_64 Linux，musl 静态链接 | Dockerfile 的 `binary` target，与 release 相同 |
| `manylinux_2_17_aarch64.manylinux2014_aarch64.musllinux_1_1_aarch64` | aarch64 Linux，musl 静态链接 | 同上，arm64 runner |
| `macosx_11_0_arm64` | Apple silicon macOS | cargo，`MACOSX_DEPLOYMENT_TARGET=11.0` |
| `macosx_10_12_x86_64` | Intel macOS | cargo，`macos-15-intel` runner，`MACOSX_DEPLOYMENT_TARGET=10.12` |
| `win_amd64` | x64 Windows | cargo，静态链接 C 运行时 |

wheel 由 `bindings/python/build_backend.py` 打包（只用标准库，不依赖 maturin 等工具），版本号取根目录 `Cargo.toml`。
没有 sdist：从源码安装要编译 Rust，需要的人可以在仓库里 `pip install bindings/python`。

每次运行都会在各自系统上安装 wheel 跑 pytest（另在 Alpine 上测 Linux wheel），再用 `twine check` 检查全部 wheel。
**只有手动触发且 `publish: true` 时才上传**，通过 [PyPI 可信发布](https://docs.pypi.org/trusted-publishers/)（OIDC），仓库里不保存 token。

## 一次性准备

1. GitHub 仓库 → Settings → Environments，新建 environment `pypi`。建议在 “Deployment protection rules” 里加上
   Required reviewers（自己），这样每次发布都要手动批准一次。
2. 在 PyPI 登录，到 Your account → Publishing → “Add a new pending publisher”（项目还不存在时用 pending publisher，
   第一次发布会创建项目），填：
   - PyPI Project Name：`fakes3-sdk`
   - Owner：`AkaraChen`
   - Repository name：`fakes3`
   - Workflow name：`python.yml`
   - Environment name：`pypi`
3. Pending publisher 不占用名字，在第一次发布之前别人仍然可以注册 `fakes3-sdk`；尽快完成第一次发布。

如果不想用可信发布，也可以改用 API token：在 PyPI 建一个 token，存为仓库 secret `PYPI_API_TOKEN`，
在 workflow 的 `pypa/gh-action-pypi-publish` 步骤加 `password: ${{ secrets.PYPI_API_TOKEN }}`，并去掉 `id-token: write`。

## 发布一个版本

1. 确认根目录 `Cargo.toml` 的 `version` 就是要发布的版本（wheel 的版本取自它），且这个版本的 python workflow 在 main 上是绿的。
2. Actions → python → Run workflow，分支选 main，勾上 `publish`。
3. 构建、测试、检查都通过后，`publish` job 在 `pypi` environment 中运行（如果配置了审批，要先批准），上传 5 个 wheel。
   这个版本已经在 PyPI 上时跳过（`skip-existing`），所以重跑不会失败。

PyPI 上的版本不能删除后重新上传同一个文件名，发布前请确认版本号。
