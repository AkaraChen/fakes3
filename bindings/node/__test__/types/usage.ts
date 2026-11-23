// Compiled with `tsc --noEmit` (npm test): checks the declarations in index.d.ts against real usage.
import { S3Client, PutObjectCommand } from '@aws-sdk/client-s3'
import { start, type ClientConfig } from '../../index.js'

export async function awaitUsing(): Promise<void> {
  await using server = await start()
  const s3 = new S3Client(server.clientConfig)
  await s3.send(new PutObjectCommand({ Bucket: 'b', Key: 'k', Body: 'v' }))
  s3.destroy()
}

export async function namedAccount(): Promise<void> {
  await using server = await start({ users: [{ name: 'alice', accessKey: 'AKALICE', secretKey: 'alice-secret' }] })
  const config: ClientConfig = server.clientConfigFor('alice')
  new S3Client(config).destroy()
}

export async function explicitStop(): Promise<void> {
  const server = await start()
  const dispose: () => Promise<void> = server[Symbol.asyncDispose]
  void dispose
  await server.stop()
}
