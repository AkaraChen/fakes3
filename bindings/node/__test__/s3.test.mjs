import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createRequire } from 'node:module'
import { S3Client, CreateBucketCommand, PutObjectCommand, GetObjectCommand, ListBucketsCommand } from '@aws-sdk/client-s3'

const { start } = createRequire(import.meta.url)('../index.js')

function client(server) {
  return new S3Client({
    endpoint: server.endpoint,
    region: 'us-east-1',
    forcePathStyle: true,
    credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
  })
}

test('bucket and object round trip', async () => {
  const server = await start()
  try {
    assert.match(server.endpoint, /^http:\/\/127\.0\.0\.1:\d+$/)
    assert.notEqual(server.port, 0)
    const s3 = client(server)
    await s3.send(new CreateBucketCommand({ Bucket: 'demo' }))
    await s3.send(new PutObjectCommand({ Bucket: 'demo', Key: 'hello.txt', Body: 'hi' }))
    const got = await s3.send(new GetObjectCommand({ Bucket: 'demo', Key: 'hello.txt' }))
    assert.equal(await got.Body.transformToString(), 'hi')
    s3.destroy()
  } finally {
    await server.stop()
  }
})

test('servers are independent', async () => {
  const [a, b] = await Promise.all([start(), start()])
  try {
    assert.notEqual(a.port, b.port)
    const sa = client(a)
    const sb = client(b)
    await sa.send(new CreateBucketCommand({ Bucket: 'only-in-a' }))
    assert.deepEqual((await sb.send(new ListBucketsCommand({}))).Buckets ?? [], [])
    sa.destroy()
    sb.destroy()
  } finally {
    await Promise.all([a.stop(), b.stop()])
  }
})

test('options: custom credentials', async () => {
  const server = await start({ accessKey: 'AK', secretKey: 'SK' })
  try {
    assert.equal(server.accessKey, 'AK')
    const good = client(server)
    await good.send(new ListBucketsCommand({}))
    good.destroy()
    const bad = new S3Client({
      endpoint: server.endpoint,
      region: 'us-east-1',
      forcePathStyle: true,
      credentials: { accessKeyId: 'AK', secretAccessKey: 'wrong' },
    })
    await assert.rejects(bad.send(new ListBucketsCommand({})), { name: 'SignatureDoesNotMatch' })
    bad.destroy()
  } finally {
    await server.stop()
  }
})

test('option noAuth accepts unsigned requests', async () => {
  const server = await start({ noAuth: true })
  try {
    const res = await fetch(server.endpoint)
    assert.equal(res.status, 200)
    assert.match(await res.text(), /ListAllMyBucketsResult/)
  } finally {
    await server.stop()
  }
})

test('stop releases the port and is idempotent', async () => {
  const server = await start()
  const { port } = server
  await server.stop()
  await server.stop()
  const again = await start({ port })
  assert.equal(again.port, port)
  await again.stop()
})

test('start fails with a readable error when the port is taken', async () => {
  const server = await start()
  try {
    await assert.rejects(start({ port: server.port }), /failed to start fakes3/)
  } finally {
    await server.stop()
  }
})

test('memoryLimit caps object data and memory() reports it', async () => {
  const server = await start({ memoryLimit: '1MiB' })
  try {
    assert.deepEqual(server.memory(), { used: 0, limit: 1024 * 1024 })
    const s3 = client(server)
    await s3.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    await s3.send(new PutObjectCommand({ Bucket: 'bucket', Key: 'small', Body: Buffer.alloc(1000) }))
    assert.ok(server.memory().used >= 1000)
    await assert.rejects(
      s3.send(new PutObjectCommand({ Bucket: 'bucket', Key: 'big', Body: Buffer.alloc(2 * 1024 * 1024) })),
      (e) => e.$metadata.httpStatusCode === 507,
    )
    s3.destroy()
  } finally {
    await server.stop()
  }
})

test('memoryLimit accepts bytes and "unlimited"; rejects garbage', async () => {
  const bytes = await start({ memoryLimit: 4096 })
  const open = await start({ memoryLimit: 'unlimited' })
  try {
    assert.equal(bytes.memory().limit, 4096)
    assert.equal(open.memory().limit, null)
    await assert.rejects(start({ memoryLimit: 'lots' }), /invalid memoryLimit/)
  } finally {
    await Promise.all([bytes.stop(), open.stop()])
  }
})

test('memory() fails after stop', async () => {
  const server = await start()
  await server.stop()
  assert.throws(() => server.memory(), /stopped/)
})
