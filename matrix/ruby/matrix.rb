# S3 SDK compatibility matrix for aws-sdk-s3 (Ruby). See ../SPEC.md.
require 'aws-sdk-s3'
require 'base64'
require 'digest'
require 'json'
require 'net/http'
require 'securerandom'
require 'tempfile'
require 'timeout'
require 'uri'
require 'zlib'

ENDPOINT = ENV.fetch('S3_ENDPOINT')
AK = ENV.fetch('S3_ACCESS_KEY', 'fakes3')
SK = ENV.fetch('S3_SECRET_KEY', 'fakes3secret')
REGION = ENV.fetch('S3_REGION', 'us-east-1')
SERVER = ENV.fetch('MATRIX_SERVER')
OUT = ENV.fetch('MATRIX_OUT')
SCENARIO_TIMEOUT = 60
MIB = 1024 * 1024

class AssertionFailed < StandardError; end

def check(cond, msg)
  raise AssertionFailed, msg unless cond
end

def eq(actual, expected, what)
  check(actual == expected, "#{what}: expected #{expected.inspect[0, 120]}, got #{actual.inspect[0, 120]}")
end

def make_client(secret = SK)
  Aws::S3::Client.new(
    endpoint: ENDPOINT, region: REGION, force_path_style: true,
    access_key_id: AK, secret_access_key: secret,
    http_open_timeout: 10, http_read_timeout: SCENARIO_TIMEOUT
  )
end

S3 = make_client
BUCKET = "m-aws-ruby-#{SecureRandom.hex(4)}"
EXTRA_BUCKETS = []

def status_of(err)
  err.respond_to?(:context) && err.context ? err.context.http_response.status_code : nil
end

# Run blk, which must raise a service error matching one of codes or statuses.
def expect_error(codes: [], statuses: [])
  yield
rescue Aws::S3::Errors::ServiceError => e
  st = status_of(e)
  return if codes.include?(e.code.to_s) || statuses.include?(st)
  raise AssertionFailed, "expected #{(codes + statuses).join('/')}, got #{e.class.name.split('::').last} (#{st}): #{e.message}"
else
  raise AssertionFailed, "expected #{(codes + statuses).join('/')}, but the call succeeded"
end

def md5hex(data) = Digest::MD5.hexdigest(data)
def unquote(etag) = etag.to_s.delete('"')
def body(resp) = resp.body.read.b

def put(key, data, **opts)
  S3.put_object(bucket: BUCKET, key: key, body: data, **opts)
end

def get(key, **opts)
  body(S3.get_object(bucket: BUCKET, key: key, **opts))
end

# Independent checksum implementations for verifying what the server returns.
def crc_table(poly, bits)
  mask = (1 << bits) - 1
  (0..255).map do |i|
    c = i
    8.times { c = (c & 1).zero? ? c >> 1 : (c >> 1) ^ poly }
    c & mask
  end
end
CRC32C_TABLE = crc_table(0x82F63B78, 32)
CRC64NVME_TABLE = crc_table(0x9A6C9329AC4BC9B5, 64)

def reflected_crc(data, table, bits)
  mask = (1 << bits) - 1
  c = mask
  data.each_byte { |b| c = table[(c ^ b) & 0xff] ^ (c >> 8) }
  c ^ mask
end

def expected_checksum(algo, data)
  case algo
  when 'CRC32' then Base64.strict_encode64([Zlib.crc32(data)].pack('N'))
  when 'CRC32C' then Base64.strict_encode64([reflected_crc(data, CRC32C_TABLE, 32)].pack('N'))
  when 'CRC64NVME' then Base64.strict_encode64([reflected_crc(data, CRC64NVME_TABLE, 64)].pack('Q>'))
  when 'SHA1' then Base64.strict_encode64(Digest::SHA1.digest(data))
  when 'SHA256' then Base64.strict_encode64(Digest::SHA256.digest(data))
  end
end

def empty_bucket(bucket)
  begin
    S3.list_object_versions(bucket: bucket).each do |page|
      objs = (page.versions + page.delete_markers).map { |v| { key: v.key, version_id: v.version_id } }
      objs.each_slice(1000) { |s| S3.delete_objects(bucket: bucket, delete: { objects: s, quiet: true }) } unless objs.empty?
    end
  rescue StandardError
    nil
  end
  S3.list_objects_v2(bucket: bucket).each do |page|
    objs = page.contents.map { |o| { key: o.key } }
    S3.delete_objects(bucket: bucket, delete: { objects: objs, quiet: true }) unless objs.empty?
  end
  begin
    S3.list_multipart_uploads(bucket: bucket).uploads.each do |u|
      S3.abort_multipart_upload(bucket: bucket, key: u.key, upload_id: u.upload_id)
    end
  rescue StandardError
    nil
  end
end

def extra_bucket(suffix)
  name = "#{BUCKET}-#{suffix}"
  S3.create_bucket(bucket: name)
  EXTRA_BUCKETS << name
  name
end

def http(method, url, body = nil)
  uri = URI(url)
  Net::HTTP.start(uri.host, uri.port, read_timeout: SCENARIO_TIMEOUT) do |h|
    req = method.new(uri.request_uri)
    if body
      req['Content-Type'] = 'application/octet-stream'
      req.body = body
    end
    h.request(req)
  end
end

def upload_parts(key, sizes)
  up = S3.create_multipart_upload(bucket: BUCKET, key: key)
  datas = sizes.map { |s| SecureRandom.random_bytes(s) }
  parts = datas.each_with_index.map do |d, i|
    r = S3.upload_part(bucket: BUCKET, key: key, upload_id: up.upload_id, part_number: i + 1, body: d)
    { part_number: i + 1, etag: r.etag }
  end
  [up.upload_id, datas, parts]
end

SCENARIOS = []
def scenario(id, &blk) = SCENARIOS << [id, blk]

scenario('create_bucket') do
  S3.create_bucket(bucket: BUCKET)
end

scenario('head_bucket') do
  S3.head_bucket(bucket: BUCKET)
end

scenario('head_bucket_missing') do
  expect_error(codes: %w[NotFound NoSuchBucket], statuses: [404]) do
    S3.head_bucket(bucket: "m-missing-#{SecureRandom.hex(6)}")
  end
end

scenario('list_buckets') do
  names = S3.list_buckets.buckets.map(&:name)
  check(names.include?(BUCKET), "bucket #{BUCKET} not in list (#{names.size} buckets)")
end

scenario('put_get_small') do
  data = 'hello world'
  r = put('small.txt', data)
  eq(unquote(r.etag), md5hex(data), 'PUT ETag')
  g = S3.get_object(bucket: BUCKET, key: 'small.txt')
  eq(body(g), data.b, 'body')
  eq(unquote(g.etag), md5hex(data), 'GET ETag')
end

scenario('put_get_empty') do
  put('empty.txt', '')
  g = S3.get_object(bucket: BUCKET, key: 'empty.txt')
  eq(g.content_length, 0, 'Content-Length')
  eq(body(g), ''.b, 'body')
end

scenario('put_get_large') do
  data = SecureRandom.random_bytes(8 * MIB)
  put('large.bin', data)
  got = get('large.bin')
  check(got == data, "8 MiB body mismatch (got #{got.bytesize} bytes)")
end

scenario('special_keys') do
  keys = ['sp ace/üñí©ødé/a+b=c&d?e#f.txt', 'a//b', "~tilde!*'()"]
  keys.each_with_index do |k, i|
    put(k, "special-#{i}")
    eq(get(k), "special-#{i}".b, "body of #{k}")
  end
  listed = []
  S3.list_objects_v2(bucket: BUCKET).each { |p| listed.concat(p.contents.map(&:key)) }
  keys.each { |k| check(listed.include?(k), "#{k.inspect} missing from listing") }
end

scenario('head_object_metadata') do
  put('meta.txt', 'meta', content_type: 'text/plain; charset=utf-8', cache_control: 'max-age=3600',
                          content_disposition: 'attachment; filename="meta.txt"', content_encoding: 'gzip',
                          metadata: { 'color' => 'Blue', 'x-y' => 'z' })
  h = S3.head_object(bucket: BUCKET, key: 'meta.txt')
  eq(h.content_type, 'text/plain; charset=utf-8', 'Content-Type')
  eq(h.cache_control, 'max-age=3600', 'Cache-Control')
  eq(h.content_disposition, 'attachment; filename="meta.txt"', 'Content-Disposition')
  eq(h.content_encoding, 'gzip', 'Content-Encoding')
  eq(h.metadata, { 'color' => 'Blue', 'x-y' => 'z' }, 'user metadata')
end

RANGE_DATA = (0...100).map { |i| (i % 256).chr }.join.b
scenario('get_range') do
  put('range.bin', RANGE_DATA)
  [['bytes=10-19', 10..19], ['bytes=-5', 95..99], ['bytes=95-', 95..99]].each do |hdr, rng|
    g = S3.get_object(bucket: BUCKET, key: 'range.bin', range: hdr)
    eq(body(g), RANGE_DATA[rng], "body for #{hdr}")
    eq(g.content_range, "bytes #{rng.first}-#{rng.last}/100", "Content-Range for #{hdr}")
  end
end

scenario('get_range_invalid') do
  put('range-invalid.bin', RANGE_DATA)
  expect_error(codes: ['InvalidRange'], statuses: [416]) { get('range-invalid.bin', range: 'bytes=200-300') }
end

scenario('get_missing_key') do
  expect_error(codes: ['NoSuchKey'], statuses: [404]) { get("missing-#{SecureRandom.hex(4)}") }
end

scenario('get_if_none_match') do
  etag = put('inm.txt', 'if-none-match').etag
  expect_error(statuses: [304]) { get('inm.txt', if_none_match: etag) }
end

scenario('get_if_match_fail') do
  put('im.txt', 'if-match')
  expect_error(codes: ['PreconditionFailed'], statuses: [412]) { get('im.txt', if_match: '"wrong"') }
end

scenario('put_if_none_match') do
  key = "pinm-#{SecureRandom.hex(4)}"
  put(key, 'first', if_none_match: '*')
  expect_error(codes: ['PreconditionFailed'], statuses: [412]) { put(key, 'second', if_none_match: '*') }
  eq(get(key), 'first'.b, 'body after rejected overwrite')
end

scenario('delete_object') do
  put('del.txt', 'x')
  S3.delete_object(bucket: BUCKET, key: 'del.txt')
  expect_error(codes: %w[NotFound NoSuchKey], statuses: [404]) { S3.head_object(bucket: BUCKET, key: 'del.txt') }
end

scenario('delete_missing_object') do
  r = S3.delete_object(bucket: BUCKET, key: "never-#{SecureRandom.hex(4)}")
  eq(r.context.http_response.status_code, 204, 'status')
end

scenario('delete_objects') do
  keys = %w[bd/1 bd/2 bd/3]
  keys.each { |k| put(k, k) }
  r = S3.delete_objects(bucket: BUCKET, delete: { objects: keys.map { |k| { key: k } } })
  check(r.errors.empty?, "errors: #{r.errors.map { |e| "#{e.key}: #{e.code}" }.join(', ')}")
  eq(r.deleted.map(&:key).sort, keys, 'deleted keys')
  keys.each do |k|
    expect_error(codes: %w[NotFound NoSuchKey], statuses: [404]) { S3.head_object(bucket: BUCKET, key: k) }
  end
end

scenario('list_v2_basic') do
  objs = { 'lb/a' => 'x', 'lb/b' => 'yy' * 10, 'lb/c' => 'zzz' * 100 }
  objs.to_a.reverse.each { |k, v| put(k, v) }
  r = S3.list_objects_v2(bucket: BUCKET, prefix: 'lb/')
  eq(r.contents.map(&:key), objs.keys, 'keys')
  eq(r.key_count, 3, 'KeyCount')
  r.contents.each do |o|
    eq(o.size, objs[o.key].bytesize, "Size of #{o.key}")
    eq(unquote(o.etag), md5hex(objs[o.key]), "ETag of #{o.key}")
  end
end

scenario('list_v2_prefix_delimiter') do
  b = extra_bucket('ld')
  %w[a/1 a/2 b/1 c].each { |k| S3.put_object(bucket: b, key: k, body: k) }
  r = S3.list_objects_v2(bucket: b, prefix: '', delimiter: '/')
  eq(r.common_prefixes.map(&:prefix), %w[a/ b/], 'CommonPrefixes')
  eq(r.contents.map(&:key), ['c'], 'Contents')
end

scenario('list_v2_pagination') do
  keys = (0...25).map { |i| format('pg/k%02d', i) }
  keys.each { |k| put(k, k) }
  got = []
  pages = 0
  token = nil
  loop do
    r = S3.list_objects_v2(bucket: BUCKET, prefix: 'pg/', max_keys: 10, continuation_token: token)
    pages += 1
    check(r.contents.size <= 10, "page #{pages} has #{r.contents.size} keys")
    got.concat(r.contents.map(&:key))
    break unless r.is_truncated
    token = r.next_continuation_token
    check(token && !token.empty?, 'truncated page without NextContinuationToken')
    check(pages < 10, 'too many pages')
  end
  eq(pages, 3, 'page count')
  eq(got, keys, 'keys across pages')
end

scenario('list_v2_start_after') do
  keys = (0...5).map { |i| "sa/k#{i}" }
  keys.each { |k| put(k, k) }
  r = S3.list_objects_v2(bucket: BUCKET, prefix: 'sa/', start_after: 'sa/k2')
  eq(r.contents.map(&:key), %w[sa/k3 sa/k4], 'keys')
end

scenario('list_v1') do
  keys = (0...5).map { |i| "l1/k#{i}" }
  keys.each { |k| put(k, k) }
  r = S3.list_objects(bucket: BUCKET, prefix: 'l1/', marker: 'l1/k1')
  eq(r.contents.map(&:key), %w[l1/k2 l1/k3 l1/k4], 'keys after marker')
  r = S3.list_objects(bucket: BUCKET, prefix: 'l1/', max_keys: 2)
  eq(r.contents.map(&:key), %w[l1/k0 l1/k1], 'first page')
  check(r.is_truncated, 'IsTruncated should be true with MaxKeys=2')
end

scenario('copy_object') do
  put('copy-src.txt', 'copy me', content_type: 'text/x-copy', metadata: { 'color' => 'Blue' })
  S3.copy_object(bucket: BUCKET, key: 'copy-dst.txt', copy_source: "#{BUCKET}/copy-src.txt")
  g = S3.get_object(bucket: BUCKET, key: 'copy-dst.txt')
  eq(body(g), 'copy me'.b, 'body')
  eq(g.content_type, 'text/x-copy', 'Content-Type')
  eq(g.metadata, { 'color' => 'Blue' }, 'metadata')
end

scenario('copy_object_replace_metadata') do
  put('copyr-src.txt', 'copy me', content_type: 'text/x-old', metadata: { 'color' => 'Blue' })
  S3.copy_object(bucket: BUCKET, key: 'copyr-dst.txt', copy_source: "#{BUCKET}/copyr-src.txt",
                 metadata_directive: 'REPLACE', content_type: 'text/x-new', metadata: { 'shape' => 'round' })
  h = S3.head_object(bucket: BUCKET, key: 'copyr-dst.txt')
  eq(h.metadata, { 'shape' => 'round' }, 'metadata')
  eq(h.content_type, 'text/x-new', 'Content-Type')
  eq(get('copyr-dst.txt'), 'copy me'.b, 'body')
end

scenario('multipart_upload') do
  id, datas, parts = upload_parts('mpu.bin', [5 * MIB, 5 * MIB, 1024])
  c = S3.complete_multipart_upload(bucket: BUCKET, key: 'mpu.bin', upload_id: id, multipart_upload: { parts: parts })
  check(unquote(c.etag) =~ /\A[0-9a-f]{32}-3\z/, "Complete ETag #{c.etag} is not <md5>-3")
  g = S3.get_object(bucket: BUCKET, key: 'mpu.bin')
  check(body(g) == datas.join, 'body mismatch')
  check(unquote(g.etag) =~ /\A[0-9a-f]{32}-3\z/, "GET ETag #{g.etag} is not <md5>-3")
end

scenario('multipart_abort') do
  id, = upload_parts('mpa.bin', [1024])
  S3.abort_multipart_upload(bucket: BUCKET, key: 'mpa.bin', upload_id: id)
  expect_error(codes: ['NoSuchUpload'], statuses: []) { S3.list_parts(bucket: BUCKET, key: 'mpa.bin', upload_id: id) }
end

scenario('multipart_list_parts') do
  id, datas, parts = upload_parts('mpl.bin', [5 * MIB, 1024])
  begin
    r = S3.list_parts(bucket: BUCKET, key: 'mpl.bin', upload_id: id)
    eq(r.parts.map(&:part_number), [1, 2], 'part numbers')
    eq(r.parts.map(&:size), datas.map(&:bytesize), 'part sizes')
    eq(r.parts.map { |p| unquote(p.etag) }, datas.map { |d| md5hex(d) }, 'part ETags')
    eq(r.parts.map { |p| unquote(p.etag) }, parts.map { |p| unquote(p[:etag]) }, 'part ETags vs UploadPart')
  ensure
    S3.abort_multipart_upload(bucket: BUCKET, key: 'mpl.bin', upload_id: id) rescue nil
  end
end

scenario('multipart_too_small') do
  id, _, parts = upload_parts('mps.bin', [1024, 1024])
  begin
    expect_error(codes: ['EntityTooSmall'], statuses: []) do
      S3.complete_multipart_upload(bucket: BUCKET, key: 'mps.bin', upload_id: id, multipart_upload: { parts: parts })
    end
  ensure
    S3.abort_multipart_upload(bucket: BUCKET, key: 'mps.bin', upload_id: id) rescue nil
  end
end

scenario('high_level_upload') do
  data = SecureRandom.random_bytes(20 * MIB)
  Tempfile.create('hl-up', binmode: true) do |src|
    src.write(data)
    src.flush
    Tempfile.create('hl-down', binmode: true) do |dst|
      if defined?(Aws::S3::TransferManager)
        tm = Aws::S3::TransferManager.new(client: S3)
        tm.upload_file(src.path, bucket: BUCKET, key: 'hl.bin', multipart_threshold: 8 * MIB)
        begin
          tm.download_file(dst.path, bucket: BUCKET, key: 'hl.bin')
        rescue StandardError => e
          raise e.class, "TransferManager#download_file: #{e.message}"
        end
      else
        obj = Aws::S3::Resource.new(client: S3).bucket(BUCKET).object('hl.bin')
        obj.upload_file(src.path, multipart_threshold: 8 * MIB)
        obj.download_file(dst.path)
      end
      got = File.binread(dst.path)
      check(got == data, "20 MiB round-trip mismatch (got #{got.bytesize} bytes)")
    end
  end
  h = S3.head_object(bucket: BUCKET, key: 'hl.bin')
  eq(h.content_length, 20 * MIB, 'Content-Length')
end

scenario('presigned_get') do
  put('presigned-get.txt', 'presigned get body')
  url = Aws::S3::Presigner.new(client: S3).presigned_url(:get_object, bucket: BUCKET, key: 'presigned-get.txt', expires_in: 300)
  r = http(Net::HTTP::Get, url)
  eq(r.code.to_i, 200, "HTTP status (#{r.body.to_s[0, 120]})")
  eq(r.body.b, 'presigned get body'.b, 'body')
end

scenario('presigned_put') do
  url = Aws::S3::Presigner.new(client: S3).presigned_url(:put_object, bucket: BUCKET, key: 'presigned-put.txt', expires_in: 300)
  r = http(Net::HTTP::Put, url, 'presigned put body')
  eq(r.code.to_i, 200, "HTTP status (#{r.body.to_s[0, 120]})")
  eq(get('presigned-put.txt'), 'presigned put body'.b, 'body')
end

%w[CRC32 CRC32C SHA1 SHA256 CRC64NVME].each do |algo|
  scenario("checksum_#{algo.downcase}") do
    key = "cks-#{algo.downcase}"
    data = "checksum payload for #{algo} " * 100
    want = expected_checksum(algo, data)
    member = :"checksum_#{algo.downcase}"
    p = put(key, data, checksum_algorithm: algo)
    eq(p.public_send(member), want, "PUT response #{member}")
    g = S3.get_object(bucket: BUCKET, key: key, checksum_mode: 'ENABLED')
    eq(body(g), data.b, 'body')
    eq(g.public_send(member), want, "GET response #{member}")
  end
end

scenario('content_md5_bad') do
  bad = Base64.strict_encode64(Digest::MD5.digest('something else'))
  expect_error(codes: %w[BadDigest InvalidDigest], statuses: [400]) { put('bad-md5.txt', 'payload', content_md5: bad) }
end

scenario('bad_credentials') do
  expect_error(codes: %w[SignatureDoesNotMatch InvalidAccessKeyId], statuses: [403]) do
    make_client('wrong-secret').list_buckets
  end
end

scenario('delete_bucket_not_empty') do
  put('not-empty.txt', 'x')
  expect_error(codes: ['BucketNotEmpty'], statuses: [409]) { S3.delete_bucket(bucket: BUCKET) }
end

scenario('versioning') do
  b = extra_bucket('ver')
  S3.put_bucket_versioning(bucket: b, versioning_configuration: { status: 'Enabled' })
  eq(S3.get_bucket_versioning(bucket: b).status, 'Enabled', 'versioning status')
  v1 = S3.put_object(bucket: b, key: 'v.txt', body: 'version one').version_id
  v2 = S3.put_object(bucket: b, key: 'v.txt', body: 'version two').version_id
  check(v1 && v2 && v1 != v2 && v1 != 'null', "PUT version ids: #{v1.inspect}, #{v2.inspect}")
  vs = S3.list_object_versions(bucket: b, prefix: 'v.txt').versions
  eq(vs.map(&:version_id).sort, [v1, v2].sort, 'listed versions')
  eq(body(S3.get_object(bucket: b, key: 'v.txt', version_id: v1)), 'version one'.b, 'old version body')
  eq(body(S3.get_object(bucket: b, key: 'v.txt')), 'version two'.b, 'latest body')
end

scenario('object_tagging') do
  put('tagged.txt', 'tags')
  tags = [{ key: 'env', value: 'test' }, { key: 'team', value: 'storage' }]
  S3.put_object_tagging(bucket: BUCKET, key: 'tagged.txt', tagging: { tag_set: tags })
  got = S3.get_object_tagging(bucket: BUCKET, key: 'tagged.txt').tag_set.map { |t| { key: t.key, value: t.value } }
  eq(got.sort_by { |t| t[:key] }, tags, 'tag set')
end

scenario('delete_bucket') do
  empty_bucket(BUCKET)
  S3.delete_bucket(bucket: BUCKET)
  expect_error(codes: %w[NotFound NoSuchBucket], statuses: [404]) { S3.head_bucket(bucket: BUCKET) }
end

def short_error(e)
  name = e.class.name.to_s.split('::').last
  msg = e.message.to_s.lines.first.to_s.strip
  if e.is_a?(Aws::S3::Errors::ServiceError)
    st = status_of(e)
    msg = "(#{st}) #{msg}" if st
  end
  "#{name}: #{msg}"[0, 300]
end

results = SCENARIOS.map do |id, blk|
  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  res = { scenario: id }
  begin
    Timeout.timeout(SCENARIO_TIMEOUT) { blk.call }
    res[:status] = 'pass'
  rescue Exception => e # rubocop:disable Lint/RescueException
    raise if e.is_a?(Interrupt)
    res[:status] = 'fail'
    res[:error] = e.is_a?(Timeout::Error) ? "Timeout: scenario exceeded #{SCENARIO_TIMEOUT}s" : short_error(e)
  end
  res[:ms] = ((Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000).round(1)
  warn format('%-30s %-4s %s', id, res[:status], res[:error])
  res
end

# Best-effort cleanup.
([BUCKET] + EXTRA_BUCKETS).each do |b|
  empty_bucket(b)
  S3.delete_bucket(bucket: b)
rescue StandardError
  nil
end

doc = { sdk: 'aws-ruby', lang: 'ruby', version: Aws::S3::GEM_VERSION, server: SERVER, results: results }
File.write(File.join(OUT, 'aws-ruby.json'), JSON.pretty_generate(doc) + "\n")
