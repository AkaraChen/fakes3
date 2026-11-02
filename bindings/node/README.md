# fakes3-sdk

[fakes3](https://github.com/AkaraChen/fakes3) 的 Node.js 绑定：一个专门给测试用的 S3 服务，全内存、无持久化，
直接跑在 Node 进程里（不起子进程、不需要 Docker）。

```bash
npm install --save-dev fakes3-sdk
```

```ts
import { start } from "fakes3-sdk";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 默认监听 127.0.0.1 的随机端口
const s3 = new S3Client({
  endpoint: server.endpoint,
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
});
await s3.send(new CreateBucketCommand({ Bucket: "demo" }));
// ...
await server.stop();
```

除了 S3 接口，`server` 上还有测试控制面：`reset()`、`snapshot()` / `restore()`、虚拟时钟 `server.clock`、
请求日志 `server.requests`。完整的选项和方法见仓库 README 的
[Node.js 绑定](https://github.com/AkaraChen/fakes3#nodejs-绑定)一节。

预编译产物覆盖 Linux 的 x64 / arm64，glibc 和 musl（Alpine）都有；macOS 和 Windows 暂不支持（服务端依赖 Linux 的 `memfd`）。

fakes3 是给测试用的，不要把它暴露在不可信的网络上，也不要拿它存真实数据。

MIT 许可。
