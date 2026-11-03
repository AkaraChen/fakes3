# 发布 fakes3-sdk 到 npm

npm 包 `fakes3-sdk` 由一个主包和 7 个平台包组成，平台包在 `@akarachen` scope 下（`package.json` 里的 `napi.packageName`）：
`@akarachen/fakes3-sdk-linux-x64-gnu`、`-linux-arm64-gnu`、`-linux-x64-musl`、`-linux-arm64-musl`、`-darwin-arm64`、`-darwin-x64`、
`-win32-x64-msvc`。不带 scope 的 `fakes3-sdk-win32-x64-msvc` 被 npm 的反垃圾检测拒绝（`Package name triggered spam detection`），
napi-rs 生态的平台包也大多放在 scope 下；0.1.2 及更早版本用的是不带 scope 的 `fakes3-sdk-linux-*`。它们都通过
[npm 可信发布](https://docs.npmjs.com/trusted-publishers)（OIDC）从 GitHub Actions 发布，仓库里不保存任何 npm token。

## 一次性准备

npm 不能给还不存在的包配置可信发布者，所以每个包名的第一个版本要由人手动发布：

1. `npm login`
2. `bindings/node/scripts/claim-npm-names.sh`：给还不存在的包名各发一个空的 `0.0.0` 占位版本（已经存在的会跳过）。
3. 为这 8 个包分别添加可信发布者（需要较新的 npm，在 11.19 上验证过；先登录）：
   ```bash
   for n in fakes3-sdk @akarachen/fakes3-sdk-linux-{x64,arm64}-{gnu,musl} @akarachen/fakes3-sdk-darwin-{arm64,x64} @akarachen/fakes3-sdk-win32-x64-msvc; do
     npm trust github "$n" --file node.yml --repo AkaraChen/fakes3 --allow-publish -y
   done
   npm trust list fakes3-sdk   # 核对
   ```
   也可以在 npmjs.com 的包页面 → Settings → Trusted publishing 里手动添加（仓库 `AkaraChen/fakes3`，workflow `node.yml`，environment 留空）。
4. 建议同时把每个包的发布权限改成「Require two-factor authentication and disallow tokens」。

新配置的可信发布者必须在 2 天内完成一次成功的发布，否则会失效，所以第 3 步做完后尽快发一个正式版本。

## 发布正式版本

1. 把 `bindings/node/package.json` 和根目录 `Cargo.toml` 的 `version` 改成新版本并合入 main。
2. 推送 tag：`git tag v0.1.0 && git push origin v0.1.0`。
   - `node.yml` 构建 7 个平台的插件、跑测试，再用 OIDC 发布平台包和主包，并自动附带 provenance。
   - `release.yml` 同时把镜像推到 `ghcr.io/akarachen/fakes3`，并把静态二进制附到 GitHub Release 上。
3. 也可以在 Actions 页面手动触发 `node`，把 `publish` 设为 true。不设置时只做 `npm publish --dry-run`。
