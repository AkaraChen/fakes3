# 发布 Java 库到 Maven Central

`bindings/java` 用 Maven 构建，坐标是 `io.github.akarachen:fakes3`（`pom.xml`）。发布走 Sonatype 的
[Central Portal](https://central.sonatype.com)，由 `.github/workflows/java-publish.yml` 完成。这个 workflow 只能手动触发，
默认是 dry run（构建、测试、打出 classes / sources / javadoc 三个 jar，不签名、不上传）；勾选 `publish` 才签名并上传，
上传后的包停在 Central Portal 里等你手动点 Publish（`autoPublish=false`）。

## 坐标

- `groupId` 建议用 `io.github.akarachen`：用 GitHub 账号登录 Central Portal 时，这个 namespace 会自动验证，不需要域名。
  如果想用自己的域名（例如 `dev.akarachen`），要先在 DNS 里加 TXT 记录验证，并同时改 `pom.xml` 的 `groupId`、
  Java 包名 `io.github.akarachen.fakes3` 和 `maven-jar-plugin` 里的 `Automatic-Module-Name`。
- `artifactId` 是 `fakes3`，库的版本号和 fakes3 的版本号保持一致。

## 一次性准备

1. 在 <https://central.sonatype.com> 用 GitHub 账号 `AkaraChen` 登录，确认 namespace `io.github.akarachen` 显示为 Verified。
2. 在 Central Portal 的 Account → Generate User Token 生成一个 user token，把它的用户名和密码分别存成仓库 secret
   `CENTRAL_USERNAME`、`CENTRAL_PASSWORD`。
3. 生成签名用的 GPG 密钥，并把公钥发到 Central 会查询的 keyserver：
   ```bash
   gpg --quick-generate-key "fakes3 release <你的邮箱>" ed25519 sign 3y
   gpg --keyserver keyserver.ubuntu.com --send-keys <KEY_ID>
   gpg --armor --export-secret-keys <KEY_ID>   # 整段存成 secret GPG_PRIVATE_KEY
   ```
   密钥的密码存成 secret `GPG_PASSPHRASE`。
4. 建议给这些 secret 配一个 GitHub environment（例如 `maven-central`）并要求审批，再在 workflow 的 job 上加
   `environment: maven-central`。

## 发布一个版本

库默认下载的 fakes3 二进制版本是 `pom.xml` 里的 `fakes3.binaryVersion`，下载地址是 GitHub Release，所以**先发 fakes3 的 release，再发 Java 库**：

1. 按平常的流程发布 fakes3（推 `v*` tag，`release.yml` 把二进制和 `.sha256` 附到 GitHub Release 上）。
2. 确认 release 上有 5 个平台的压缩包后，把 `pom.xml` 的 `<version>` 和 `<fakes3.binaryVersion>` 改成新版本，并更新
   `bindings/java/README*.md` 和根目录 README 里示例的版本号，合入 main。（如果先改 `fakes3.binaryVersion` 再发 release，
   `java.yml` 的测试会因为下载不到二进制而失败。）
3. 在 Actions 页面手动运行 `java-publish`，先不勾选 `publish` 跑一次 dry run，再勾选 `publish` 跑一次。
4. 在 Central Portal 的 Deployments 页面检查上传的内容，点 Publish。通常十几分钟后能在 Maven Central 上搜到。
5. 去掉 `bindings/java/README*.md` 和根目录 README 里“还没有发布到 Maven Central”的提示（只需第一次）。

## 本地验证

```bash
cd bindings/java
mvn verify                     # 测试会下载 fakes3 二进制，或者用 FAKES3_BINARY 指定一个
mvn -Prelease verify -Dgpg.skip   # 检查 release profile 能打包（不签名）
```
