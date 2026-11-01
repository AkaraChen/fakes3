# 发布 fakes3-sdk 到 npm

npm 包 `fakes3-sdk` 由一个主包和 4 个平台包组成：`fakes3-sdk-linux-x64-gnu`、`fakes3-sdk-linux-arm64-gnu`、
`fakes3-sdk-linux-x64-musl`、`fakes3-sdk-linux-arm64-musl`。它们都通过
[npm 可信发布](https://docs.npmjs.com/trusted-publishers)（OIDC）从 GitHub Actions 发布，仓库里不保存任何 npm token。

## 一次性准备

npm 不能给还不存在的包配置可信发布者，所以每个包名的第一个版本要由人手动发布：

1. `npm login`
2. `bindings/node/scripts/claim-npm-names.sh`：给 5 个包名各发一个空的 `0.0.0` 占位版本。
3. 在 npmjs.com 上为这 5 个包分别添加可信发布者（包页面 → Settings → Trusted publishing）：
   - Organization / user：`AkaraChen`
   - Repository：`fakes3`
   - Workflow filename：`node.yml`
   - Environment：留空
4. 建议同时把每个包的发布权限改成「Require two-factor authentication and disallow tokens」。

新配置的可信发布者必须在 2 天内完成一次成功的发布，否则会失效，所以第 3 步做完后尽快发一个正式版本。

## 发布正式版本

1. 把 `bindings/node/package.json` 和根目录 `Cargo.toml` 的 `version` 改成新版本并合入 main。
2. 推送 tag：`git tag v0.1.0 && git push origin v0.1.0`。
   - `node.yml` 构建 4 个平台的插件、跑测试，再用 OIDC 发布平台包和主包，并自动附带 provenance。
   - `release.yml` 同时把镜像推到 `ghcr.io/akarachen/fakes3`，并把静态二进制附到 GitHub Release 上。
3. 也可以在 Actions 页面手动触发 `node`，把 `publish` 设为 true。不设置时只做 `npm publish --dry-run`。
