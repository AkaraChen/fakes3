import assert from 'node:assert/strict'
import http from 'node:http'
import { test } from 'node:test'
import { createRequire } from 'node:module'
import {
  S3Client,
  CreateBucketCommand,
  PutObjectCommand,
  GetObjectCommand,
  HeadObjectCommand,
  ListBucketsCommand,
  GetPublicAccessBlockCommand,
} from '@aws-sdk/client-s3'

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

test('bucketDefaults: "aws" blocks public access on new buckets; garbage is rejected', async () => {
  const aws = await start({ bucketDefaults: 'aws' })
  const legacy = await start()
  try {
    const a = client(aws)
    await a.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    const { PublicAccessBlockConfiguration: pab } = await a.send(new GetPublicAccessBlockCommand({ Bucket: 'bucket' }))
    assert.deepEqual(pab, {
      BlockPublicAcls: true,
      IgnorePublicAcls: true,
      BlockPublicPolicy: true,
      RestrictPublicBuckets: true,
    })
    const l = client(legacy)
    await l.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    await assert.rejects(l.send(new GetPublicAccessBlockCommand({ Bucket: 'bucket' })), { name: 'NoSuchPublicAccessBlockConfiguration' })
    a.destroy()
    l.destroy()
    await assert.rejects(start({ bucketDefaults: '2023' }), /invalid bucketDefaults/)
  } finally {
    await Promise.all([aws.stop(), legacy.stop()])
  }
})

test('memoryLimit accepts bytes and "unlimited"; rejects garbage', async () => {
  const bytes = await start({ memoryLimit: 4096 })
  const open = await start({ memoryLimit: 'unlimited' })
  try {
    assert.equal(bytes.memory().limit, 4096)
    assert.equal(open.memory().limit, undefined)
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

async function withServer(options, fn) {
  const server = await start(options)
  const s3 = client(server)
  try {
    await fn(server, s3)
  } finally {
    s3.destroy()
    await server.stop()
  }
}

const buckets = async (s3) => (await s3.send(new ListBucketsCommand({}))).Buckets?.map((b) => b.Name) ?? []

test('reset clears data and the request log, keeps snapshots unless asked', async () => {
  await withServer(undefined, async (server, s3) => {
    await s3.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    server.snapshot('base')
    assert.ok(server.requests.count() > 0)
    server.reset()
    assert.deepEqual(await buckets(s3), [])
    // The ListBuckets above is the only request since the reset.
    assert.deepEqual(server.requests.list().map((r) => r.operation), ['ListBuckets'])
    assert.equal(server.snapshots().length, 1)
    server.reset({ snapshots: true })
    assert.equal(server.snapshots().length, 0)
  })
})

test('snapshot, restore, list and delete', async () => {
  await withServer(undefined, async (server, s3) => {
    await s3.send(new CreateBucketCommand({ Bucket: 'seeded' }))
    await s3.send(new PutObjectCommand({ Bucket: 'seeded', Key: 'k', Body: 'v1' }))
    const info = server.snapshot('seeded')
    assert.equal(info.name, 'seeded')
    assert.ok(info.created instanceof Date)
    assert.equal(info.buckets, 1)
    assert.equal(info.objects, 1)
    assert.match(server.snapshot().name, /^snap-\d+$/)
    assert.throws(() => server.snapshot('seeded'), /already|exists/i)

    await s3.send(new PutObjectCommand({ Bucket: 'seeded', Key: 'k', Body: 'v2' }))
    await s3.send(new CreateBucketCommand({ Bucket: 'later' }))
    assert.equal(server.restore('seeded').name, 'seeded')
    assert.deepEqual(await buckets(s3), ['seeded'])
    const got = await s3.send(new GetObjectCommand({ Bucket: 'seeded', Key: 'k' }))
    assert.equal(await got.Body.transformToString(), 'v1')
    server.restore('seeded') // snapshots are not consumed

    assert.equal(server.snapshots().length, 2)
    server.deleteSnapshot('seeded')
    assert.equal(server.snapshots().length, 1)
    assert.throws(() => server.restore('seeded'), /seeded/)
    assert.throws(() => server.deleteSnapshot('seeded'), /seeded/)
  })
})

test('clock: freeze, advance, set, unfreeze, reset', async () => {
  await withServer(undefined, async (server, s3) => {
    const clock = server.clock
    assert.equal(clock.isFrozen(), false)
    assert.equal(clock.isShifted(), false)
    assert.ok(Math.abs(clock.now() - Date.now()) < 5000)

    clock.freeze(new Date('2030-01-02T03:04:05Z'))
    assert.equal(clock.isFrozen(), true)
    assert.equal(clock.now().toISOString(), '2030-01-02T03:04:05.000Z')
    await s3.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    await s3.send(new PutObjectCommand({ Bucket: 'bucket', Key: 'k', Body: 'x' }))
    const head = await s3.send(new HeadObjectCommand({ Bucket: 'bucket', Key: 'k' }))
    assert.equal(head.LastModified.toISOString(), '2030-01-02T03:04:05.000Z')

    clock.advance('31d')
    assert.equal(clock.now().toISOString(), '2030-02-02T03:04:05.000Z')
    clock.advance(-1000 * 60 * 60 * 24) // milliseconds
    assert.equal(clock.now().toISOString(), '2030-02-01T03:04:05.000Z')
    clock.set(new Date('2031-01-01T00:00:00Z'))
    assert.equal(clock.now().toISOString(), '2031-01-01T00:00:00.000Z')

    clock.unfreeze()
    assert.equal(clock.isFrozen(), false)
    assert.equal(clock.isShifted(), true)
    assert.ok(clock.now() >= new Date('2031-01-01T00:00:00Z'))

    assert.throws(() => clock.advance('soon'), /invalid duration/)
    clock.reset()
    assert.equal(clock.isShifted(), false)
    assert.ok(Math.abs(clock.now() - Date.now()) < 5000)

    clock.freeze()
    server.reset() // restores the host clock
    assert.equal(clock.isFrozen(), false)
  })
})

test('requests: list, filter, count, clear, configure', async () => {
  await withServer(undefined, async (server, s3) => {
    const log = server.requests
    await s3.send(new CreateBucketCommand({ Bucket: 'bucket' }))
    await s3.send(new PutObjectCommand({ Bucket: 'bucket', Key: 'a/1', Body: 'hello' }))
    await s3.send(new PutObjectCommand({ Bucket: 'bucket', Key: 'b/2', Body: 'hi' }))
    await assert.rejects(s3.send(new GetObjectCommand({ Bucket: 'bucket', Key: 'missing' })))

    const all = log.list()
    assert.deepEqual(all.map((r) => r.operation), ['CreateBucket', 'PutObject', 'PutObject', 'GetObject'])
    assert.deepEqual(all.map((r) => r.seq), [...all.map((r) => r.seq)].sort((a, b) => a - b))
    const put = all[1]
    assert.equal(put.method, 'PUT')
    assert.equal(put.bucket, 'bucket')
    assert.equal(put.key, 'a/1')
    assert.equal(put.status, 200)
    assert.equal(put.bytesIn, 5)
    assert.ok(put.time instanceof Date)

    assert.equal(log.list({ operation: 'putobject' }).length, 2)
    assert.deepEqual(log.list({ keyPrefix: 'b/' }).map((r) => r.key), ['b/2'])
    assert.deepEqual(log.list({ status: 404 }).map((r) => r.operation), ['GetObject'])
    assert.deepEqual(log.list({ status: 'error' }).map((r) => r.status), [404])
    assert.equal(log.list({ status: '2xx' }).length, 3)
    assert.equal(log.list({ method: 'put' }).length, 3) // CreateBucket is a PUT too
    assert.deepEqual(log.list({ after: all[2].seq }).map((r) => r.operation), ['GetObject'])
    assert.equal(log.list({ limit: 1 })[0].operation, 'GetObject') // the most recent match
    assert.equal(log.count({ operation: 'PutObject', limit: 1 }), 2)
    assert.throws(() => log.list({ status: 'weird' }), /invalid status/)

    log.clear()
    assert.equal(log.count(), 0)
    await s3.send(new ListBucketsCommand({}))
    assert.ok(log.list()[0].seq > all[3].seq)

    log.configure({ enabled: false })
    await s3.send(new ListBucketsCommand({}))
    assert.equal(log.count(), 1)
    log.configure({ enabled: true, capacity: 2 })
    for (let i = 0; i < 3; i++) await s3.send(new ListBucketsCommand({}))
    assert.equal(log.count(), 2)
  })
})

test('requestLog option turns the log off', async () => {
  await withServer({ requestLog: false }, async (server, s3) => {
    await s3.send(new ListBucketsCommand({}))
    assert.equal(server.requests.count(), 0)
  })
})

test('control methods fail after stop', async () => {
  const server = await start()
  const { clock, requests } = server
  await server.stop()
  assert.throws(() => server.reset(), /stopped/)
  assert.throws(() => server.snapshot(), /stopped/)
  assert.throws(() => clock.now(), /stopped/)
  assert.throws(() => requests.list(), /stopped/)
})

function request(server, method, path, { host, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: server.host, port: server.port, method, path, headers: host ? { host } : {} },
      (res) => {
        const chunks = []
        res.on('data', (c) => chunks.push(c))
        res.on('end', () => resolve({ status: res.statusCode, body: Buffer.concat(chunks).toString() }))
      },
    )
    req.on('error', reject)
    req.end(body)
  })
}

test('domains enables virtual-host addressing', async () => {
  // noAuth lets a bare HTTP request stand in for a client with a custom Host header.
  const server = await start({ noAuth: true, domains: ['s3.test'] })
  try {
    assert.equal((await request(server, 'PUT', '/vhb')).status, 200)
    assert.equal((await request(server, 'PUT', '/obj', { host: 'vhb.s3.test', body: 'data' })).status, 200)
    assert.equal((await request(server, 'GET', '/vhb/obj')).body, 'data') // path style still works
    assert.equal((await request(server, 'GET', '/obj', { host: 'vhb.s3.test' })).body, 'data')
  } finally {
    await server.stop()
  }
})
