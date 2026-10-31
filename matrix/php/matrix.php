<?php
// S3 SDK compatibility matrix for aws/aws-sdk-php. See ../SPEC.md.
declare(strict_types=1);

require __DIR__ . '/vendor/autoload.php';

use Aws\Exception\AwsException;
use Aws\S3\MultipartUploader;
use Aws\S3\S3Client;

const MIB = 1048576;
const SCENARIO_TIMEOUT = 60;

$ENDPOINT = getenv('S3_ENDPOINT') ?: die("S3_ENDPOINT required\n");
$AK = getenv('S3_ACCESS_KEY') ?: 'fakes3';
$SK = getenv('S3_SECRET_KEY') ?: 'fakes3secret';
$REGION = getenv('S3_REGION') ?: 'us-east-1';
$SERVER = getenv('MATRIX_SERVER') ?: die("MATRIX_SERVER required\n");
$OUT = getenv('MATRIX_OUT') ?: die("MATRIX_OUT required\n");

class AssertionFailed extends RuntimeException {}
class Skip extends RuntimeException {}

function check(bool $cond, string $msg): void
{
    if (!$cond) {
        throw new AssertionFailed($msg);
    }
}

function show($v): string
{
    return substr(var_export($v, true), 0, 120);
}

function eq($actual, $expected, string $what): void
{
    check($actual === $expected, "$what: expected " . show($expected) . ', got ' . show($actual));
}

function makeClient(string $secret): S3Client
{
    global $ENDPOINT, $AK, $REGION;
    return new S3Client([
        'version' => 'latest',
        'region' => $REGION,
        'endpoint' => $ENDPOINT,
        'use_path_style_endpoint' => true,
        'credentials' => ['key' => $AK, 'secret' => $secret],
        'http' => ['connect_timeout' => 10, 'timeout' => SCENARIO_TIMEOUT],
    ]);
}

$s3 = makeClient($SK);
$BUCKET = 'm-aws-php-' . bin2hex(random_bytes(4));
$EXTRA = [];

/** Run $fn, which must fail with one of the given error codes / HTTP statuses. */
function expectError(callable $fn, array $codes = [], array $statuses = []): void
{
    try {
        $fn();
    } catch (AwsException $e) {
        $st = $e->getStatusCode();
        if (in_array($e->getAwsErrorCode(), $codes, true) || in_array($st, $statuses, true)) {
            return;
        }
        throw new AssertionFailed('expected ' . implode('/', array_merge($codes, $statuses))
            . ', got ' . ($e->getAwsErrorCode() ?? 'no code') . " ($st): " . firstLine($e->getAwsErrorMessage() ?? $e->getMessage()));
    }
    throw new AssertionFailed('expected ' . implode('/', array_merge($codes, $statuses)) . ', but the call succeeded');
}

function firstLine(string $s): string
{
    return trim(strtok($s, "\n") ?: '');
}

function unquote(?string $etag): string
{
    return str_replace('"', '', (string) $etag);
}

function put(string $key, string $data, array $opts = [])
{
    global $s3, $BUCKET;
    return $s3->putObject(['Bucket' => $BUCKET, 'Key' => $key, 'Body' => $data] + $opts);
}

function getObj(string $key, array $opts = [])
{
    global $s3, $BUCKET;
    return $s3->getObject(['Bucket' => $BUCKET, 'Key' => $key] + $opts);
}

function get(string $key, array $opts = []): string
{
    return (string) getObj($key, $opts)['Body'];
}

function headObj(string $key)
{
    global $s3, $BUCKET;
    return $s3->headObject(['Bucket' => $BUCKET, 'Key' => $key]);
}

// --- independent checksum implementations -----------------------------------
function crc64nvme(string $data): int
{
    static $table = null;
    $poly = 0x1A6C9329AC4BC9B5 | PHP_INT_MIN; // 0x9A6C9329AC4BC9B5
    if ($table === null) {
        $table = [];
        for ($i = 0; $i < 256; $i++) {
            $c = $i;
            for ($j = 0; $j < 8; $j++) {
                $c = ($c & 1) ? ((($c >> 1) & PHP_INT_MAX) ^ $poly) : (($c >> 1) & PHP_INT_MAX);
            }
            $table[$i] = $c;
        }
    }
    $c = -1;
    $n = strlen($data);
    for ($i = 0; $i < $n; $i++) {
        $c = $table[($c ^ ord($data[$i])) & 0xff] ^ (($c >> 8) & 0x00FFFFFFFFFFFFFF);
    }
    return ~$c;
}

function expectedChecksum(string $algo, string $data): string
{
    return base64_encode(match ($algo) {
        'CRC32' => hash('crc32b', $data, true),
        'CRC32C' => hash('crc32c', $data, true),
        'SHA1' => hash('sha1', $data, true),
        'SHA256' => hash('sha256', $data, true),
        'CRC64NVME' => pack('J', crc64nvme($data)),
    });
}

function emptyBucket(string $bucket): void
{
    global $s3;
    try {
        foreach ($s3->getPaginator('ListObjectVersions', ['Bucket' => $bucket]) as $page) {
            $objs = [];
            foreach (array_merge($page['Versions'] ?? [], $page['DeleteMarkers'] ?? []) as $v) {
                $objs[] = ['Key' => $v['Key'], 'VersionId' => $v['VersionId']];
            }
            if ($objs) {
                $s3->deleteObjects(['Bucket' => $bucket, 'Delete' => ['Objects' => $objs, 'Quiet' => true]]);
            }
        }
    } catch (Throwable) {
    }
    foreach ($s3->getPaginator('ListObjectsV2', ['Bucket' => $bucket]) as $page) {
        $objs = array_map(fn($o) => ['Key' => $o['Key']], $page['Contents'] ?? []);
        if ($objs) {
            $s3->deleteObjects(['Bucket' => $bucket, 'Delete' => ['Objects' => $objs, 'Quiet' => true]]);
        }
    }
    try {
        foreach ($s3->listMultipartUploads(['Bucket' => $bucket])['Uploads'] ?? [] as $u) {
            $s3->abortMultipartUpload(['Bucket' => $bucket, 'Key' => $u['Key'], 'UploadId' => $u['UploadId']]);
        }
    } catch (Throwable) {
    }
}

function extraBucket(string $suffix): string
{
    global $s3, $BUCKET, $EXTRA;
    $name = "$BUCKET-$suffix";
    $s3->createBucket(['Bucket' => $name]);
    $EXTRA[] = $name;
    return $name;
}

/** Plain HTTP request via ext-curl. Returns [status, body]. */
function httpRequest(string $method, string $url, ?string $body = null): array
{
    $ch = curl_init($url);
    curl_setopt_array($ch, [
        CURLOPT_CUSTOMREQUEST => $method,
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => SCENARIO_TIMEOUT,
    ]);
    if ($body !== null) {
        curl_setopt($ch, CURLOPT_POSTFIELDS, $body);
        curl_setopt($ch, CURLOPT_HTTPHEADER, ['Content-Type: application/octet-stream', 'Expect:']);
    }
    $resp = curl_exec($ch);
    if ($resp === false) {
        throw new RuntimeException('curl: ' . curl_error($ch));
    }
    return [curl_getinfo($ch, CURLINFO_RESPONSE_CODE), $resp];
}

function uploadParts(string $key, array $sizes): array
{
    global $s3, $BUCKET;
    $id = $s3->createMultipartUpload(['Bucket' => $BUCKET, 'Key' => $key])['UploadId'];
    $datas = [];
    $parts = [];
    foreach ($sizes as $i => $size) {
        $d = random_bytes($size);
        $datas[] = $d;
        $r = $s3->uploadPart(['Bucket' => $BUCKET, 'Key' => $key, 'UploadId' => $id, 'PartNumber' => $i + 1, 'Body' => $d]);
        $parts[] = ['PartNumber' => $i + 1, 'ETag' => $r['ETag']];
    }
    return [$id, $datas, $parts];
}

function abortQuietly(string $key, string $id): void
{
    global $s3, $BUCKET;
    try {
        $s3->abortMultipartUpload(['Bucket' => $BUCKET, 'Key' => $key, 'UploadId' => $id]);
    } catch (Throwable) {
    }
}

$RANGE_DATA = '';
for ($i = 0; $i < 100; $i++) {
    $RANGE_DATA .= chr($i);
}

$scenarios = [];

$scenarios['create_bucket'] = function () use ($s3, $BUCKET) {
    $s3->createBucket(['Bucket' => $BUCKET]);
};

$scenarios['head_bucket'] = function () use ($s3, $BUCKET) {
    $s3->headBucket(['Bucket' => $BUCKET]);
};

$scenarios['head_bucket_missing'] = function () use ($s3) {
    expectError(fn() => $s3->headBucket(['Bucket' => 'm-missing-' . bin2hex(random_bytes(6))]), ['NotFound', 'NoSuchBucket'], [404]);
};

$scenarios['list_buckets'] = function () use ($s3, $BUCKET) {
    $names = array_column($s3->listBuckets()['Buckets'] ?? [], 'Name');
    check(in_array($BUCKET, $names, true), "bucket $BUCKET not in list (" . count($names) . ' buckets)');
};

$scenarios['put_get_small'] = function () {
    $data = 'hello world';
    $r = put('small.txt', $data);
    eq(unquote($r['ETag']), md5($data), 'PUT ETag');
    $g = getObj('small.txt');
    eq((string) $g['Body'], $data, 'body');
    eq(unquote($g['ETag']), md5($data), 'GET ETag');
};

$scenarios['put_get_empty'] = function () {
    put('empty.txt', '');
    $g = getObj('empty.txt');
    eq((int) $g['ContentLength'], 0, 'Content-Length');
    eq((string) $g['Body'], '', 'body');
};

$scenarios['put_get_large'] = function () {
    $data = random_bytes(8 * MIB);
    put('large.bin', $data);
    $got = get('large.bin');
    check($got === $data, '8 MiB body mismatch (got ' . strlen($got) . ' bytes)');
};

$scenarios['special_keys'] = function () use ($s3, $BUCKET) {
    $keys = ['sp ace/üñí©ødé/a+b=c&d?e#f.txt', 'a//b', "~tilde!*'()"];
    foreach ($keys as $i => $k) {
        put($k, "special-$i");
        eq(get($k), "special-$i", "body of $k");
    }
    $listed = [];
    foreach ($s3->getPaginator('ListObjectsV2', ['Bucket' => $BUCKET]) as $page) {
        foreach ($page['Contents'] ?? [] as $o) {
            $listed[] = $o['Key'];
        }
    }
    foreach ($keys as $k) {
        check(in_array($k, $listed, true), show($k) . ' missing from listing');
    }
};

$scenarios['head_object_metadata'] = function () {
    put('meta.txt', 'meta', [
        'ContentType' => 'text/plain; charset=utf-8',
        'CacheControl' => 'max-age=3600',
        'ContentDisposition' => 'attachment; filename="meta.txt"',
        'ContentEncoding' => 'gzip',
        'Metadata' => ['color' => 'Blue', 'x-y' => 'z'],
    ]);
    $h = headObj('meta.txt');
    eq($h['ContentType'], 'text/plain; charset=utf-8', 'Content-Type');
    eq($h['CacheControl'], 'max-age=3600', 'Cache-Control');
    eq($h['ContentDisposition'], 'attachment; filename="meta.txt"', 'Content-Disposition');
    eq($h['ContentEncoding'], 'gzip', 'Content-Encoding');
    $md = $h['Metadata'];
    ksort($md);
    eq($md, ['color' => 'Blue', 'x-y' => 'z'], 'user metadata');
};

$scenarios['get_range'] = function () use ($RANGE_DATA) {
    put('range.bin', $RANGE_DATA);
    foreach ([['bytes=10-19', 10, 19], ['bytes=-5', 95, 99], ['bytes=95-', 95, 99]] as [$hdr, $a, $b]) {
        $g = getObj('range.bin', ['Range' => $hdr]);
        eq((string) $g['Body'], substr($RANGE_DATA, $a, $b - $a + 1), "body for $hdr");
        eq($g['ContentRange'], "bytes $a-$b/100", "Content-Range for $hdr");
    }
};

$scenarios['get_range_invalid'] = function () use ($RANGE_DATA) {
    put('range-invalid.bin', $RANGE_DATA);
    expectError(fn() => get('range-invalid.bin', ['Range' => 'bytes=200-300']), ['InvalidRange'], [416]);
};

$scenarios['get_missing_key'] = function () {
    expectError(fn() => get('missing-' . bin2hex(random_bytes(4))), ['NoSuchKey'], [404]);
};

$scenarios['get_if_none_match'] = function () {
    $etag = put('inm.txt', 'if-none-match')['ETag'];
    // Guzzle does not treat 304 as an error, so the SDK returns a normal result.
    try {
        $r = getObj('inm.txt', ['IfNoneMatch' => $etag]);
    } catch (AwsException $e) {
        eq($e->getStatusCode(), 304, 'status (' . $e->getAwsErrorCode() . ')');
        return;
    }
    eq($r['@metadata']['statusCode'], 304, 'status');
};

$scenarios['get_if_match_fail'] = function () {
    put('im.txt', 'if-match');
    expectError(fn() => get('im.txt', ['IfMatch' => '"wrong"']), ['PreconditionFailed'], [412]);
};

$scenarios['put_if_none_match'] = function () {
    $key = 'pinm-' . bin2hex(random_bytes(4));
    put($key, 'first', ['IfNoneMatch' => '*']);
    expectError(fn() => put($key, 'second', ['IfNoneMatch' => '*']), ['PreconditionFailed'], [412]);
    eq(get($key), 'first', 'body after rejected overwrite');
};

$scenarios['delete_object'] = function () use ($s3, $BUCKET) {
    put('del.txt', 'x');
    $s3->deleteObject(['Bucket' => $BUCKET, 'Key' => 'del.txt']);
    expectError(fn() => headObj('del.txt'), ['NotFound', 'NoSuchKey'], [404]);
};

$scenarios['delete_missing_object'] = function () use ($s3, $BUCKET) {
    $r = $s3->deleteObject(['Bucket' => $BUCKET, 'Key' => 'never-' . bin2hex(random_bytes(4))]);
    eq($r['@metadata']['statusCode'], 204, 'status');
};

$scenarios['delete_objects'] = function () use ($s3, $BUCKET) {
    $keys = ['bd/1', 'bd/2', 'bd/3'];
    foreach ($keys as $k) {
        put($k, $k);
    }
    $r = $s3->deleteObjects(['Bucket' => $BUCKET, 'Delete' => ['Objects' => array_map(fn($k) => ['Key' => $k], $keys)]]);
    check(empty($r['Errors']), 'errors: ' . json_encode($r['Errors']));
    $deleted = array_column($r['Deleted'] ?? [], 'Key');
    sort($deleted);
    eq($deleted, $keys, 'deleted keys');
    foreach ($keys as $k) {
        expectError(fn() => headObj($k), ['NotFound', 'NoSuchKey'], [404]);
    }
};

$scenarios['list_v2_basic'] = function () use ($s3, $BUCKET) {
    $objs = ['lb/a' => 'x', 'lb/b' => str_repeat('yy', 10), 'lb/c' => str_repeat('zzz', 100)];
    foreach (array_reverse($objs, true) as $k => $v) {
        put($k, $v);
    }
    $r = $s3->listObjectsV2(['Bucket' => $BUCKET, 'Prefix' => 'lb/']);
    $contents = $r['Contents'] ?? [];
    eq(array_column($contents, 'Key'), array_keys($objs), 'keys');
    eq((int) $r['KeyCount'], 3, 'KeyCount');
    foreach ($contents as $o) {
        eq((int) $o['Size'], strlen($objs[$o['Key']]), "Size of {$o['Key']}");
        eq(unquote($o['ETag']), md5($objs[$o['Key']]), "ETag of {$o['Key']}");
    }
};

$scenarios['list_v2_prefix_delimiter'] = function () use ($s3) {
    $b = extraBucket('ld');
    foreach (['a/1', 'a/2', 'b/1', 'c'] as $k) {
        $s3->putObject(['Bucket' => $b, 'Key' => $k, 'Body' => $k]);
    }
    $r = $s3->listObjectsV2(['Bucket' => $b, 'Prefix' => '', 'Delimiter' => '/']);
    eq(array_column($r['CommonPrefixes'] ?? [], 'Prefix'), ['a/', 'b/'], 'CommonPrefixes');
    eq(array_column($r['Contents'] ?? [], 'Key'), ['c'], 'Contents');
};

$scenarios['list_v2_pagination'] = function () use ($s3, $BUCKET) {
    $keys = [];
    for ($i = 0; $i < 25; $i++) {
        $keys[] = sprintf('pg/k%02d', $i);
        put($keys[$i], $keys[$i]);
    }
    $got = [];
    $pages = 0;
    $token = null;
    while (true) {
        $args = ['Bucket' => $BUCKET, 'Prefix' => 'pg/', 'MaxKeys' => 10];
        if ($token !== null) {
            $args['ContinuationToken'] = $token;
        }
        $r = $s3->listObjectsV2($args);
        $pages++;
        $c = $r['Contents'] ?? [];
        check(count($c) <= 10, "page $pages has " . count($c) . ' keys');
        array_push($got, ...array_column($c, 'Key'));
        if (!$r['IsTruncated']) {
            break;
        }
        $token = $r['NextContinuationToken'];
        check(!empty($token), 'truncated page without NextContinuationToken');
        check($pages < 10, 'too many pages');
    }
    eq($pages, 3, 'page count');
    eq($got, $keys, 'keys across pages');
};

$scenarios['list_v2_start_after'] = function () use ($s3, $BUCKET) {
    for ($i = 0; $i < 5; $i++) {
        put("sa/k$i", "k$i");
    }
    $r = $s3->listObjectsV2(['Bucket' => $BUCKET, 'Prefix' => 'sa/', 'StartAfter' => 'sa/k2']);
    eq(array_column($r['Contents'] ?? [], 'Key'), ['sa/k3', 'sa/k4'], 'keys');
};

$scenarios['list_v1'] = function () use ($s3, $BUCKET) {
    for ($i = 0; $i < 5; $i++) {
        put("l1/k$i", "k$i");
    }
    $r = $s3->listObjects(['Bucket' => $BUCKET, 'Prefix' => 'l1/', 'Marker' => 'l1/k1']);
    eq(array_column($r['Contents'] ?? [], 'Key'), ['l1/k2', 'l1/k3', 'l1/k4'], 'keys after marker');
    $r = $s3->listObjects(['Bucket' => $BUCKET, 'Prefix' => 'l1/', 'MaxKeys' => 2]);
    eq(array_column($r['Contents'] ?? [], 'Key'), ['l1/k0', 'l1/k1'], 'first page');
    check((bool) $r['IsTruncated'], 'IsTruncated should be true with MaxKeys=2');
};

$scenarios['copy_object'] = function () use ($s3, $BUCKET) {
    put('copy-src.txt', 'copy me', ['ContentType' => 'text/x-copy', 'Metadata' => ['color' => 'Blue']]);
    $s3->copyObject(['Bucket' => $BUCKET, 'Key' => 'copy-dst.txt', 'CopySource' => "$BUCKET/copy-src.txt"]);
    $g = getObj('copy-dst.txt');
    eq((string) $g['Body'], 'copy me', 'body');
    eq($g['ContentType'], 'text/x-copy', 'Content-Type');
    eq($g['Metadata'], ['color' => 'Blue'], 'metadata');
};

$scenarios['copy_object_replace_metadata'] = function () use ($s3, $BUCKET) {
    put('copyr-src.txt', 'copy me', ['ContentType' => 'text/x-old', 'Metadata' => ['color' => 'Blue']]);
    $s3->copyObject([
        'Bucket' => $BUCKET, 'Key' => 'copyr-dst.txt', 'CopySource' => "$BUCKET/copyr-src.txt",
        'MetadataDirective' => 'REPLACE', 'ContentType' => 'text/x-new', 'Metadata' => ['shape' => 'round'],
    ]);
    $h = headObj('copyr-dst.txt');
    eq($h['Metadata'], ['shape' => 'round'], 'metadata');
    eq($h['ContentType'], 'text/x-new', 'Content-Type');
    eq(get('copyr-dst.txt'), 'copy me', 'body');
};

$scenarios['multipart_upload'] = function () use ($s3, $BUCKET) {
    [$id, $datas, $parts] = uploadParts('mpu.bin', [5 * MIB, 5 * MIB, 1024]);
    $c = $s3->completeMultipartUpload(['Bucket' => $BUCKET, 'Key' => 'mpu.bin', 'UploadId' => $id, 'MultipartUpload' => ['Parts' => $parts]]);
    check((bool) preg_match('/^[0-9a-f]{32}-3$/', unquote($c['ETag'])), "Complete ETag {$c['ETag']} is not <md5>-3");
    $g = getObj('mpu.bin');
    check((string) $g['Body'] === implode('', $datas), 'body mismatch');
    check((bool) preg_match('/^[0-9a-f]{32}-3$/', unquote($g['ETag'])), "GET ETag {$g['ETag']} is not <md5>-3");
};

$scenarios['multipart_abort'] = function () use ($s3, $BUCKET) {
    [$id] = uploadParts('mpa.bin', [1024]);
    $s3->abortMultipartUpload(['Bucket' => $BUCKET, 'Key' => 'mpa.bin', 'UploadId' => $id]);
    expectError(fn() => $s3->listParts(['Bucket' => $BUCKET, 'Key' => 'mpa.bin', 'UploadId' => $id]), ['NoSuchUpload']);
};

$scenarios['multipart_list_parts'] = function () use ($s3, $BUCKET) {
    [$id, $datas, $parts] = uploadParts('mpl.bin', [5 * MIB, 1024]);
    try {
        $r = $s3->listParts(['Bucket' => $BUCKET, 'Key' => 'mpl.bin', 'UploadId' => $id]);
        $ps = $r['Parts'] ?? [];
        eq(array_map('intval', array_column($ps, 'PartNumber')), [1, 2], 'part numbers');
        eq(array_map('intval', array_column($ps, 'Size')), array_map('strlen', $datas), 'part sizes');
        eq(array_map('unquote', array_column($ps, 'ETag')), array_map('md5', $datas), 'part ETags');
        eq(array_map('unquote', array_column($ps, 'ETag')), array_map('unquote', array_column($parts, 'ETag')), 'part ETags vs UploadPart');
    } finally {
        abortQuietly('mpl.bin', $id);
    }
};

$scenarios['multipart_too_small'] = function () use ($s3, $BUCKET) {
    [$id, , $parts] = uploadParts('mps.bin', [1024, 1024]);
    try {
        expectError(fn() => $s3->completeMultipartUpload([
            'Bucket' => $BUCKET, 'Key' => 'mps.bin', 'UploadId' => $id, 'MultipartUpload' => ['Parts' => $parts],
        ]), ['EntityTooSmall']);
    } finally {
        abortQuietly('mps.bin', $id);
    }
};

$scenarios['high_level_upload'] = function () use ($s3, $BUCKET) {
    $data = random_bytes(20 * MIB);
    $src = tempnam(sys_get_temp_dir(), 'hl-up');
    $dst = tempnam(sys_get_temp_dir(), 'hl-down');
    try {
        file_put_contents($src, $data);
        $up = new MultipartUploader($s3, $src, ['bucket' => $BUCKET, 'key' => 'hl.bin', 'part_size' => 8 * MIB, 'concurrency' => 3]);
        $res = $up->upload();
        check((bool) preg_match('/^[0-9a-f]{32}-3$/', unquote($res['ETag'] ?? '')), 'upload ETag ' . show($res['ETag'] ?? null) . ' is not <md5>-3');
        getObj('hl.bin', ['SaveAs' => $dst]);
        $got = file_get_contents($dst);
        check($got === $data, '20 MiB round-trip mismatch (got ' . strlen($got) . ' bytes)');
    } finally {
        @unlink($src);
        @unlink($dst);
    }
};

$scenarios['presigned_get'] = function () use ($s3, $BUCKET) {
    put('presigned-get.txt', 'presigned get body');
    $cmd = $s3->getCommand('GetObject', ['Bucket' => $BUCKET, 'Key' => 'presigned-get.txt']);
    $url = (string) $s3->createPresignedRequest($cmd, '+5 minutes')->getUri();
    [$st, $body] = httpRequest('GET', $url);
    eq($st, 200, 'HTTP status (' . substr($body, 0, 120) . ')');
    eq($body, 'presigned get body', 'body');
};

$scenarios['presigned_put'] = function () use ($s3, $BUCKET) {
    $cmd = $s3->getCommand('PutObject', ['Bucket' => $BUCKET, 'Key' => 'presigned-put.txt']);
    $url = (string) $s3->createPresignedRequest($cmd, '+5 minutes')->getUri();
    [$st, $body] = httpRequest('PUT', $url, 'presigned put body');
    eq($st, 200, 'HTTP status (' . substr($body, 0, 120) . ')');
    eq(get('presigned-put.txt'), 'presigned put body', 'body');
};

foreach (['CRC32', 'CRC32C', 'SHA1', 'SHA256', 'CRC64NVME'] as $algo) {
    $scenarios['checksum_' . strtolower($algo)] = function () use ($algo) {
        $key = 'cks-' . strtolower($algo);
        $data = str_repeat("checksum payload for $algo ", 100);
        $want = expectedChecksum($algo, $data);
        $member = "Checksum$algo";
        $opts = ['ChecksumAlgorithm' => $algo];
        // aws-sdk-php can only compute CRC32C (and never CRC64NVME) with the
        // optional awscrt extension; otherwise supply the value ourselves via
        // the modelled Checksum* member, which the SDK sends as-is.
        if (($algo === 'CRC32C' && !extension_loaded('awscrt')) || $algo === 'CRC64NVME') {
            $opts[$member] = $want;
        }
        $p = put($key, $data, $opts);
        eq($p[$member] ?? null, $want, "PUT response $member");
        $g = getObj($key, ['ChecksumMode' => 'ENABLED']);
        eq((string) $g['Body'], $data, 'body');
        eq($g[$member] ?? null, $want, "GET response $member");
    };
}

$scenarios['content_md5_bad'] = function () {
    $bad = base64_encode(md5('something else', true));
    expectError(fn() => put('bad-md5.txt', 'payload', ['ContentMD5' => $bad]), ['BadDigest', 'InvalidDigest'], [400]);
};

$scenarios['bad_credentials'] = function () {
    expectError(fn() => makeClient('wrong-secret')->listBuckets(), ['SignatureDoesNotMatch', 'InvalidAccessKeyId'], [403]);
};

$scenarios['delete_bucket_not_empty'] = function () use ($s3, $BUCKET) {
    put('not-empty.txt', 'x');
    expectError(fn() => $s3->deleteBucket(['Bucket' => $BUCKET]), ['BucketNotEmpty'], [409]);
};

$scenarios['versioning'] = function () use ($s3) {
    $b = extraBucket('ver');
    $s3->putBucketVersioning(['Bucket' => $b, 'VersioningConfiguration' => ['Status' => 'Enabled']]);
    eq($s3->getBucketVersioning(['Bucket' => $b])['Status'] ?? null, 'Enabled', 'versioning status');
    $v1 = $s3->putObject(['Bucket' => $b, 'Key' => 'v.txt', 'Body' => 'version one'])['VersionId'] ?? null;
    $v2 = $s3->putObject(['Bucket' => $b, 'Key' => 'v.txt', 'Body' => 'version two'])['VersionId'] ?? null;
    check($v1 && $v2 && $v1 !== $v2 && $v1 !== 'null', 'PUT version ids: ' . show([$v1, $v2]));
    $listed = array_column($s3->listObjectVersions(['Bucket' => $b, 'Prefix' => 'v.txt'])['Versions'] ?? [], 'VersionId');
    sort($listed);
    $want = [$v1, $v2];
    sort($want);
    eq($listed, $want, 'listed versions');
    eq((string) $s3->getObject(['Bucket' => $b, 'Key' => 'v.txt', 'VersionId' => $v1])['Body'], 'version one', 'old version body');
    eq((string) $s3->getObject(['Bucket' => $b, 'Key' => 'v.txt'])['Body'], 'version two', 'latest body');
};

$scenarios['object_tagging'] = function () use ($s3, $BUCKET) {
    put('tagged.txt', 'tags');
    $tags = [['Key' => 'env', 'Value' => 'test'], ['Key' => 'team', 'Value' => 'storage']];
    $s3->putObjectTagging(['Bucket' => $BUCKET, 'Key' => 'tagged.txt', 'Tagging' => ['TagSet' => $tags]]);
    $got = $s3->getObjectTagging(['Bucket' => $BUCKET, 'Key' => 'tagged.txt'])['TagSet'] ?? [];
    usort($got, fn($a, $b) => strcmp($a['Key'], $b['Key']));
    eq($got, $tags, 'tag set');
};

$scenarios['delete_bucket'] = function () use ($s3, $BUCKET) {
    emptyBucket($BUCKET);
    $s3->deleteBucket(['Bucket' => $BUCKET]);
    expectError(fn() => $s3->headBucket(['Bucket' => $BUCKET]), ['NotFound', 'NoSuchBucket'], [404]);
};

function shortError(Throwable $e): string
{
    $name = (new ReflectionClass($e))->getShortName();
    if ($e instanceof AwsException && $e->getStatusCode() !== null) {
        $msg = ($e->getAwsErrorCode() ?? 'Error') . ' (' . $e->getStatusCode() . '): '
            . firstLine((string) ($e->getAwsErrorMessage() ?: $e->getMessage()));
    } else {
        $msg = firstLine($e->getMessage());
    }
    return substr("$name: $msg", 0, 300);
}

$results = [];
foreach ($scenarios as $id => $fn) {
    $t0 = hrtime(true);
    $res = ['scenario' => $id];
    try {
        $fn();
        $res['status'] = 'pass';
    } catch (Skip $e) {
        $res['status'] = 'skip';
        $res['error'] = $e->getMessage();
    } catch (Throwable $e) {
        $res['status'] = 'fail';
        $res['error'] = shortError($e);
    }
    $res['ms'] = round((hrtime(true) - $t0) / 1e6, 1);
    if (($res['status'] === 'pass') && $res['ms'] > SCENARIO_TIMEOUT * 1000) {
        $res['status'] = 'fail';
        $res['error'] = 'Timeout: scenario exceeded ' . SCENARIO_TIMEOUT . 's';
    }
    fprintf(STDERR, "%-30s %-4s %s\n", $id, $res['status'], $res['error'] ?? '');
    $results[] = $res;
}

foreach (array_merge([$BUCKET], $EXTRA) as $b) {
    try {
        emptyBucket($b);
        $s3->deleteBucket(['Bucket' => $b]);
    } catch (Throwable) {
    }
}

$doc = [
    'sdk' => 'aws-php',
    'lang' => 'php',
    'version' => \Aws\Sdk::VERSION,
    'server' => $SERVER,
    'results' => $results,
];
file_put_contents("$OUT/aws-php.json", json_encode($doc, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE) . "\n");
