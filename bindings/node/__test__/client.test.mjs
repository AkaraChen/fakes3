// `await using` and `clientConfig`. Uses `await using` syntax, so it needs Node 24 or later.
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { createRequire } from 'node:module'
import {
  S3Client,
  CreateBucketCommand,
  PutObjectCommand,
  GetObjectCommand,
  ListBucketsCommand,
} from '@aws-sdk/client-s3'

const { start, FakeS3Server } = createRequire(import.meta.url)('../index.js')

/** Fails unless `port` is free: starts a server on it and stops it again. */
async function assertPortFree(port) {
  const probe = await start({ port })
  assert.equal(probe.port, port)
  await probe.stop()
}

test('Symbol.asyncDispose is stop', () => {
  assert.equal(typeof Symbol.asyncDispose, 'symbol')
  assert.equal(FakeS3Server.prototype[Symbol.asyncDispose], FakeS3Server.prototype.stop)
  assert.equal(Object.getOwnPropertyDescriptor(FakeS3Server.prototype, Symbol.asyncDispose).enumerable, false)
})

test('await using stops the server when the scope ends', async () => {
  let port
  {
    await using server = await start()
    port = server.port
    await assert.rejects(start({ port }), /failed to start fakes3/)
  }
  await assertPortFree(port)
})

test('await using stops the server when the scope throws', async () => {
  let port
  await assert.rejects(async () => {
    await using server = await start()
    port = server.port
    throw new Error('boom')
  }, /boom/)
  await assertPortFree(port)
})

test('disposing after stop() is fine', async () => {
  await using server = await start()
  await server.stop()
})

test('clientConfig builds a working S3Client', async () => {
  await using server = await start({ auth: true })
  assert.deepEqual(server.clientConfig, {
    endpoint: server.endpoint,
    region: 'us-east-1',
    forcePathStyle: true,
    credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
  })
  const s3 = new S3Client(server.clientConfig)
  try {
    await s3.send(new CreateBucketCommand({ Bucket: 'demo' }))
    await s3.send(new PutObjectCommand({ Bucket: 'demo', Key: 'hello.txt', Body: 'hi' }))
    const got = await s3.send(new GetObjectCommand({ Bucket: 'demo', Key: 'hello.txt' }))
    assert.equal(await got.Body.transformToString(), 'hi')
  } finally {
    s3.destroy()
  }
})

test('clientConfigFor signs as the given account', async () => {
  await using server = await start({
    auth: true,
    users: [{ name: 'alice', accessKey: 'AKALICE', secretKey: 'alice-secret' }],
  })
  const alice = server.users.find((u) => u.name === 'alice')
  const config = server.clientConfigFor('alice')
  assert.deepEqual(config.credentials, { accessKeyId: 'AKALICE', secretAccessKey: 'alice-secret' })
  assert.deepEqual(server.clientConfigFor('AKALICE'), config)
  assert.deepEqual(server.clientConfigFor('fakes3'), server.clientConfig)
  assert.throws(() => server.clientConfigFor('nobody'), /nobody/)

  const asAlice = new S3Client(config)
  const asDefault = new S3Client(server.clientConfig)
  try {
    await asAlice.send(new CreateBucketCommand({ Bucket: 'alices' }))
    const listed = await asAlice.send(new ListBucketsCommand({}))
    assert.equal(listed.Owner.ID, alice.canonicalId)
    assert.equal(listed.Owner.DisplayName, 'alice')
    assert.deepEqual(listed.Buckets.map((b) => b.Name), ['alices'])
    // The default account does not own alice's bucket.
    const others = await asDefault.send(new ListBucketsCommand({}))
    assert.notEqual(others.Owner.ID, alice.canonicalId)
    assert.deepEqual(others.Buckets ?? [], [])
  } finally {
    asAlice.destroy()
    asDefault.destroy()
  }
})
