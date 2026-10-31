package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net/url"
	"slices"
	"strings"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/feature/s3/manager"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/s3/types"
)

func newV2Client(cfg Config, secret string) *s3.Client {
	ac := aws.Config{
		Region:      cfg.Region,
		Credentials: credentials.NewStaticCredentialsProvider(cfg.AccessKey, secret, ""),
	}
	return s3.NewFromConfig(ac, func(o *s3.Options) {
		o.BaseEndpoint = aws.String(cfg.Endpoint)
		o.UsePathStyle = true
	})
}

// ---------- admin helpers (setup/cleanup for SDKs that lack bucket APIs) ----------

func adminEnsureBucket(ctx context.Context, cfg Config, bucket string) error {
	c := newV2Client(cfg, cfg.SecretKey)
	if _, err := c.HeadBucket(ctx, &s3.HeadBucketInput{Bucket: &bucket}); err == nil {
		return nil
	}
	_, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: &bucket})
	return err
}

func adminEmptyBucket(ctx context.Context, c *s3.Client, bucket string) error {
	// Abort pending multipart uploads.
	if mu, err := c.ListMultipartUploads(ctx, &s3.ListMultipartUploadsInput{Bucket: &bucket}); err == nil {
		for _, u := range mu.Uploads {
			_, _ = c.AbortMultipartUpload(ctx, &s3.AbortMultipartUploadInput{Bucket: &bucket, Key: u.Key, UploadId: u.UploadId})
		}
	}
	// Delete all versions + delete markers (works for unversioned buckets too, where supported).
	if vs, err := c.ListObjectVersions(ctx, &s3.ListObjectVersionsInput{Bucket: &bucket}); err == nil {
		for _, v := range vs.Versions {
			_, _ = c.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &bucket, Key: v.Key, VersionId: v.VersionId})
		}
		for _, v := range vs.DeleteMarkers {
			_, _ = c.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &bucket, Key: v.Key, VersionId: v.VersionId})
		}
	}
	p := s3.NewListObjectsV2Paginator(c, &s3.ListObjectsV2Input{Bucket: &bucket})
	for p.HasMorePages() {
		page, err := p.NextPage(ctx)
		if err != nil {
			return err
		}
		for _, o := range page.Contents {
			if _, err := c.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &bucket, Key: o.Key}); err != nil {
				return err
			}
		}
	}
	return nil
}

func adminNukeBucket(ctx context.Context, cfg Config, bucket string) error {
	c := newV2Client(cfg, cfg.SecretKey)
	if _, err := c.HeadBucket(ctx, &s3.HeadBucketInput{Bucket: &bucket}); err != nil {
		return nil
	}
	_ = adminEmptyBucket(ctx, c, bucket)
	_, err := c.DeleteBucket(ctx, &s3.DeleteBucketInput{Bucket: &bucket})
	return err
}

// ---------- aws-sdk-go-v2 ----------

type V2SDK struct{}

func (V2SDK) Name() string   { return "aws-sdk-go-v2" }
func (V2SDK) Module() string { return "github.com/aws/aws-sdk-go-v2/service/s3" }

func v2Get(ctx context.Context, c *s3.Client, bucket, key string) ([]byte, *s3.GetObjectOutput, error) {
	out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &bucket, Key: &key})
	if err != nil {
		return nil, nil, err
	}
	defer out.Body.Close()
	b, err := io.ReadAll(out.Body)
	return b, out, err
}

func v2Put(ctx context.Context, c *s3.Client, bucket, key string, data []byte) (*s3.PutObjectOutput, error) {
	return c.PutObject(ctx, &s3.PutObjectInput{Bucket: &bucket, Key: &key, Body: bytes.NewReader(data)})
}

func v2ListAll(ctx context.Context, c *s3.Client, bucket, prefix string) ([]types.Object, error) {
	var all []types.Object
	p := s3.NewListObjectsV2Paginator(c, &s3.ListObjectsV2Input{Bucket: &bucket, Prefix: &prefix})
	for p.HasMorePages() {
		page, err := p.NextPage(ctx)
		if err != nil {
			return nil, err
		}
		all = append(all, page.Contents...)
	}
	return all, nil
}

func v2Checksum(out *s3.GetObjectOutput, alg string) *string {
	switch alg {
	case "CRC32":
		return out.ChecksumCRC32
	case "CRC32C":
		return out.ChecksumCRC32C
	case "SHA1":
		return out.ChecksumSHA1
	case "SHA256":
		return out.ChecksumSHA256
	case "CRC64NVME":
		return out.ChecksumCRC64NVME
	}
	return nil
}

func (V2SDK) Scenarios(env *Env) map[string]func(ctx context.Context) error {
	c := newV2Client(env.Config, env.SecretKey)
	b := env.Bucket
	m := map[string]func(ctx context.Context) error{}

	m["create_bucket"] = func(ctx context.Context) error {
		_, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: &b})
		return err
	}
	m["head_bucket"] = func(ctx context.Context) error {
		_, err := c.HeadBucket(ctx, &s3.HeadBucketInput{Bucket: &b})
		return err
	}
	m["head_bucket_missing"] = func(ctx context.Context) error {
		_, err := c.HeadBucket(ctx, &s3.HeadBucketInput{Bucket: aws.String("m-missing-" + randHex(6))})
		return expectErr("HeadBucket", err, []int{404}, "NotFound", "NoSuchBucket")
	}
	m["list_buckets"] = func(ctx context.Context) error {
		out, err := c.ListBuckets(ctx, &s3.ListBucketsInput{})
		if err != nil {
			return err
		}
		for _, bk := range out.Buckets {
			if aws.ToString(bk.Name) == b {
				return nil
			}
		}
		return failf("bucket %s not in ListBuckets (%d buckets)", b, len(out.Buckets))
	}
	m["put_get_small"] = func(ctx context.Context) error {
		data := []byte("hello world")
		po, err := v2Put(ctx, c, b, "small.txt", data)
		if err != nil {
			return err
		}
		got, gout, err := v2Get(ctx, c, b, "small.txt")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data),
			eqStr("PUT ETag", unquote(aws.ToString(po.ETag)), md5Hex(data)),
			eqStr("GET ETag", unquote(aws.ToString(gout.ETag)), md5Hex(data)))
	}
	m["put_get_empty"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "empty", nil); err != nil {
			return err
		}
		got, out, err := v2Get(ctx, c, b, "empty")
		if err != nil {
			return err
		}
		if len(got) != 0 || aws.ToInt64(out.ContentLength) != 0 {
			return failf("expected empty body, got %d bytes, Content-Length %d", len(got), aws.ToInt64(out.ContentLength))
		}
		return nil
	}
	m["put_get_large"] = func(ctx context.Context) error {
		data := randBytes(8 << 20)
		if _, err := v2Put(ctx, c, b, "large.bin", data); err != nil {
			return err
		}
		got, _, err := v2Get(ctx, c, b, "large.bin")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	m["special_keys"] = func(ctx context.Context) error {
		for _, k := range specialKeys {
			data := []byte("key:" + k)
			if _, err := v2Put(ctx, c, b, k, data); err != nil {
				return fmt.Errorf("PUT %q: %w", k, err)
			}
			got, _, err := v2Get(ctx, c, b, k)
			if err != nil {
				return fmt.Errorf("GET %q: %w", k, err)
			}
			if err := eqBytes("GET "+k, got, data); err != nil {
				return err
			}
		}
		objs, err := v2ListAll(ctx, c, b, "")
		if err != nil {
			return err
		}
		var keys []string
		for _, o := range objs {
			keys = append(keys, aws.ToString(o.Key))
		}
		for _, k := range specialKeys {
			if !slices.Contains(keys, k) {
				return failf("key %q missing from list", k)
			}
		}
		return nil
	}
	m["head_object_metadata"] = func(ctx context.Context) error {
		k := "meta.txt"
		_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: &k, Body: strings.NewReader("meta"),
			ContentType: aws.String(mdContentType), CacheControl: aws.String(mdCacheControl),
			ContentDisposition: aws.String(mdContentDisposition), ContentEncoding: aws.String(mdContentEncoding),
			Metadata: metaIn})
		if err != nil {
			return err
		}
		h, err := c.HeadObject(ctx, &s3.HeadObjectInput{Bucket: &b, Key: &k})
		if err != nil {
			return err
		}
		return firstErr(
			eqStr("Content-Type", aws.ToString(h.ContentType), mdContentType),
			eqStr("Cache-Control", aws.ToString(h.CacheControl), mdCacheControl),
			eqStr("Content-Disposition", aws.ToString(h.ContentDisposition), mdContentDisposition),
			eqStr("Content-Encoding", aws.ToString(h.ContentEncoding), mdContentEncoding),
			checkMeta(h.Metadata))
	}
	m["get_range"] = func(ctx context.Context) error {
		data := randBytes(100)
		if _, err := v2Put(ctx, c, b, "range.bin", data); err != nil {
			return err
		}
		cases := []struct {
			rng, cr string
			want    []byte
		}{
			{"bytes=10-19", "bytes 10-19/100", data[10:20]},
			{"bytes=-5", "bytes 95-99/100", data[95:]},
			{"bytes=95-", "bytes 95-99/100", data[95:]},
		}
		for _, tc := range cases {
			out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("range.bin"), Range: aws.String(tc.rng)})
			if err != nil {
				return fmt.Errorf("%s: %w", tc.rng, err)
			}
			got, err := io.ReadAll(out.Body)
			out.Body.Close()
			if err != nil {
				return err
			}
			if err := firstErr(eqBytes(tc.rng, got, tc.want), eqStr(tc.rng+" Content-Range", aws.ToString(out.ContentRange), tc.cr)); err != nil {
				return err
			}
		}
		return nil
	}
	m["get_range_invalid"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "range-inv.bin", randBytes(100)); err != nil {
			return err
		}
		out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("range-inv.bin"), Range: aws.String("bytes=200-300")})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET bytes=200-300", err, []int{416}, "InvalidRange")
	}
	m["get_missing_key"] = func(ctx context.Context) error {
		_, _, err := v2Get(ctx, c, b, "missing-"+randHex(4))
		return expectErr("GET", err, []int{404}, "NoSuchKey")
	}
	m["get_if_none_match"] = func(ctx context.Context) error {
		po, err := v2Put(ctx, c, b, "cond.txt", []byte("conditional"))
		if err != nil {
			return err
		}
		out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("cond.txt"), IfNoneMatch: po.ETag})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET If-None-Match", err, []int{304}, "NotModified")
	}
	m["get_if_match_fail"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "cond2.txt", []byte("conditional")); err != nil {
			return err
		}
		out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("cond2.txt"), IfMatch: aws.String(`"wrong"`)})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET If-Match", err, []int{412}, "PreconditionFailed")
	}
	m["put_if_none_match"] = func(ctx context.Context) error {
		k := "ifnm-" + randHex(4)
		put := func() error {
			_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: &k, Body: strings.NewReader("x"), IfNoneMatch: aws.String("*")})
			return err
		}
		if err := put(); err != nil {
			return fmt.Errorf("first PUT (new key): %w", err)
		}
		return expectErr("second PUT", put(), []int{412}, "PreconditionFailed")
	}
	m["delete_object"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "del.txt", []byte("x")); err != nil {
			return err
		}
		if _, err := c.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &b, Key: aws.String("del.txt")}); err != nil {
			return err
		}
		_, err := c.HeadObject(ctx, &s3.HeadObjectInput{Bucket: &b, Key: aws.String("del.txt")})
		return expectErr("HEAD after delete", err, []int{404}, "NotFound", "NoSuchKey")
	}
	m["delete_missing_object"] = func(ctx context.Context) error {
		_, err := c.DeleteObject(ctx, &s3.DeleteObjectInput{Bucket: &b, Key: aws.String("never-" + randHex(4))})
		return err
	}
	m["delete_objects"] = func(ctx context.Context) error {
		keys := []string{"bulk/1", "bulk/2", "bulk/3"}
		var ids []types.ObjectIdentifier
		for _, k := range keys {
			if _, err := v2Put(ctx, c, b, k, []byte(k)); err != nil {
				return err
			}
			ids = append(ids, types.ObjectIdentifier{Key: aws.String(k)})
		}
		out, err := c.DeleteObjects(ctx, &s3.DeleteObjectsInput{Bucket: &b, Delete: &types.Delete{Objects: ids}})
		if err != nil {
			return err
		}
		if len(out.Errors) > 0 {
			return failf("DeleteObjects errors: %s %s", aws.ToString(out.Errors[0].Code), aws.ToString(out.Errors[0].Message))
		}
		if len(out.Deleted) != 3 {
			return failf("Deleted has %d entries, want 3", len(out.Deleted))
		}
		objs, err := v2ListAll(ctx, c, b, "bulk/")
		if err != nil {
			return err
		}
		if len(objs) != 0 {
			return failf("%d objects remain", len(objs))
		}
		return nil
	}
	m["list_v2_basic"] = func(ctx context.Context) error {
		keys := []string{"lb/a", "lb/b", "lb/c"}
		datas := [][]byte{randBytes(1), randBytes(22), randBytes(333)}
		for i := len(keys) - 1; i >= 0; i-- {
			if _, err := v2Put(ctx, c, b, keys[i], datas[i]); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("lb/")})
		if err != nil {
			return err
		}
		if len(out.Contents) != 3 {
			return failf("got %d objects, want 3", len(out.Contents))
		}
		for i, o := range out.Contents {
			if err := firstErr(eqStr("key", aws.ToString(o.Key), keys[i]),
				eqStr("etag "+keys[i], unquote(aws.ToString(o.ETag)), md5Hex(datas[i]))); err != nil {
				return err
			}
			if aws.ToInt64(o.Size) != int64(len(datas[i])) {
				return failf("size %s: got %d want %d", keys[i], aws.ToInt64(o.Size), len(datas[i]))
			}
		}
		return nil
	}
	m["list_v2_prefix_delimiter"] = func(ctx context.Context) error {
		db := b + "-d"
		if _, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: &db}); err != nil {
			return err
		}
		for _, k := range []string{"a/1", "a/2", "b/1", "c"} {
			if _, err := v2Put(ctx, c, db, k, []byte(k)); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2(ctx, &s3.ListObjectsV2Input{Bucket: &db, Prefix: aws.String(""), Delimiter: aws.String("/")})
		if err != nil {
			return err
		}
		var cps, cs []string
		for _, p := range out.CommonPrefixes {
			cps = append(cps, aws.ToString(p.Prefix))
		}
		for _, o := range out.Contents {
			cs = append(cs, aws.ToString(o.Key))
		}
		return firstErr(eqStr("CommonPrefixes", fmt.Sprint(cps), "[a/ b/]"), eqStr("Contents", fmt.Sprint(cs), "[c]"))
	}
	m["list_v2_pagination"] = func(ctx context.Context) error {
		for _, k := range paginationKeys("pg/") {
			if _, err := v2Put(ctx, c, b, k, []byte("p")); err != nil {
				return err
			}
		}
		var all []string
		pages := 0
		var token *string
		for {
			out, err := c.ListObjectsV2(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("pg/"), MaxKeys: aws.Int32(10), ContinuationToken: token})
			if err != nil {
				return err
			}
			pages++
			for _, o := range out.Contents {
				all = append(all, aws.ToString(o.Key))
			}
			if !aws.ToBool(out.IsTruncated) {
				break
			}
			if pages > 10 {
				return failf("too many pages")
			}
			token = out.NextContinuationToken
		}
		if pages != 3 || len(all) != 25 {
			return failf("got %d pages / %d keys, want 3 / 25", pages, len(all))
		}
		return sortedUnique(all)
	}
	m["list_v2_start_after"] = func(ctx context.Context) error {
		keys := []string{"sa/0", "sa/1", "sa/2", "sa/3", "sa/4"}
		for _, k := range keys {
			if _, err := v2Put(ctx, c, b, k, []byte("s")); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("sa/"), StartAfter: aws.String("sa/2")})
		if err != nil {
			return err
		}
		var got []string
		for _, o := range out.Contents {
			got = append(got, aws.ToString(o.Key))
		}
		return eqStr("keys", fmt.Sprint(got), "[sa/3 sa/4]")
	}
	m["list_v1"] = func(ctx context.Context) error {
		keys := []string{"l1/0", "l1/1", "l1/2", "l1/3", "l1/4"}
		for _, k := range keys {
			if _, err := v2Put(ctx, c, b, k, []byte("v1")); err != nil {
				return err
			}
		}
		// Page through with MaxKeys=2 using Marker.
		var got []string
		marker := aws.String("l1/0")
		for i := 0; i < 10; i++ {
			out, err := c.ListObjects(ctx, &s3.ListObjectsInput{Bucket: &b, Prefix: aws.String("l1/"), Marker: marker, MaxKeys: aws.Int32(2)})
			if err != nil {
				return err
			}
			for _, o := range out.Contents {
				got = append(got, aws.ToString(o.Key))
			}
			if !aws.ToBool(out.IsTruncated) {
				break
			}
			if len(out.Contents) == 0 {
				return failf("truncated page with no contents")
			}
			marker = out.Contents[len(out.Contents)-1].Key
		}
		return eqStr("keys after marker", fmt.Sprint(got), "[l1/1 l1/2 l1/3 l1/4]")
	}
	m["copy_object"] = func(ctx context.Context) error {
		data := []byte("copy me")
		_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("cp/src"), Body: bytes.NewReader(data),
			ContentType: aws.String("text/x-copy"), Metadata: metaIn})
		if err != nil {
			return err
		}
		if _, err := c.CopyObject(ctx, &s3.CopyObjectInput{Bucket: &b, Key: aws.String("cp/dst"), CopySource: aws.String(b + "/cp/src")}); err != nil {
			return err
		}
		got, out, err := v2Get(ctx, c, b, "cp/dst")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("Content-Type", aws.ToString(out.ContentType), "text/x-copy"), checkMeta(out.Metadata))
	}
	m["copy_object_replace_metadata"] = func(ctx context.Context) error {
		_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("cpr/src"), Body: strings.NewReader("x"),
			ContentType: aws.String("text/x-old"), Metadata: metaIn})
		if err != nil {
			return err
		}
		_, err = c.CopyObject(ctx, &s3.CopyObjectInput{Bucket: &b, Key: aws.String("cpr/dst"), CopySource: aws.String(b + "/cpr/src"),
			MetadataDirective: types.MetadataDirectiveReplace, ContentType: aws.String("text/x-new"),
			Metadata: map[string]string{"color": "Red", "fresh": "yes"}})
		if err != nil {
			return err
		}
		h, err := c.HeadObject(ctx, &s3.HeadObjectInput{Bucket: &b, Key: aws.String("cpr/dst")})
		if err != nil {
			return err
		}
		return firstErr(eqStr("Content-Type", aws.ToString(h.ContentType), "text/x-new"),
			eqStr("metadata", fmt.Sprint(h.Metadata), "map[color:Red fresh:yes]"))
	}
	mpParts := func(ctx context.Context, key string, sizes []int) (string, [][]byte, []types.CompletedPart, error) {
		cr, err := c.CreateMultipartUpload(ctx, &s3.CreateMultipartUploadInput{Bucket: &b, Key: &key})
		if err != nil {
			return "", nil, nil, err
		}
		var datas [][]byte
		var parts []types.CompletedPart
		for i, sz := range sizes {
			d := randBytes(sz)
			up, err := c.UploadPart(ctx, &s3.UploadPartInput{Bucket: &b, Key: &key, UploadId: cr.UploadId,
				PartNumber: aws.Int32(int32(i + 1)), Body: bytes.NewReader(d)})
			if err != nil {
				return *cr.UploadId, nil, nil, err
			}
			datas = append(datas, d)
			parts = append(parts, types.CompletedPart{ETag: up.ETag, PartNumber: aws.Int32(int32(i + 1))})
		}
		return *cr.UploadId, datas, parts, nil
	}
	abort := func(key, id string) {
		if id != "" {
			_, _ = c.AbortMultipartUpload(context.Background(), &s3.AbortMultipartUploadInput{Bucket: &b, Key: &key, UploadId: &id})
		}
	}
	m["multipart_upload"] = func(ctx context.Context) error {
		k := "mp/obj"
		id, datas, parts, err := mpParts(ctx, k, []int{5 << 20, 5 << 20, 1024})
		if err != nil {
			abort(k, id)
			return err
		}
		out, err := c.CompleteMultipartUpload(ctx, &s3.CompleteMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id,
			MultipartUpload: &types.CompletedMultipartUpload{Parts: parts}})
		if err != nil {
			abort(k, id)
			return err
		}
		got, _, err := v2Get(ctx, c, b, k)
		if err != nil {
			return err
		}
		if err := eqBytes("GET", got, bytes.Join(datas, nil)); err != nil {
			return err
		}
		if !isMultipartETag(aws.ToString(out.ETag), 3) {
			return failf("multipart ETag %q not of form <hex>-3", aws.ToString(out.ETag))
		}
		return nil
	}
	m["multipart_abort"] = func(ctx context.Context) error {
		k := "mp/abort"
		id, _, _, err := mpParts(ctx, k, []int{1024})
		if err != nil {
			abort(k, id)
			return err
		}
		if _, err := c.AbortMultipartUpload(ctx, &s3.AbortMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id}); err != nil {
			return err
		}
		_, err = c.ListParts(ctx, &s3.ListPartsInput{Bucket: &b, Key: &k, UploadId: &id})
		return expectErr("ListParts after abort", err, []int{404}, "NoSuchUpload")
	}
	m["multipart_list_parts"] = func(ctx context.Context) error {
		k := "mp/lp"
		sizes := []int{5 << 20, 1024}
		id, datas, parts, err := mpParts(ctx, k, sizes)
		defer abort(k, id)
		if err != nil {
			return err
		}
		out, err := c.ListParts(ctx, &s3.ListPartsInput{Bucket: &b, Key: &k, UploadId: &id})
		if err != nil {
			return err
		}
		if len(out.Parts) != 2 {
			return failf("ListParts returned %d parts, want 2", len(out.Parts))
		}
		for i, p := range out.Parts {
			if aws.ToInt32(p.PartNumber) != int32(i+1) || aws.ToInt64(p.Size) != int64(sizes[i]) ||
				unquote(aws.ToString(p.ETag)) != unquote(aws.ToString(parts[i].ETag)) || unquote(aws.ToString(p.ETag)) != md5Hex(datas[i]) {
				return failf("part %d: number=%d size=%d etag=%s", i+1, aws.ToInt32(p.PartNumber), aws.ToInt64(p.Size), aws.ToString(p.ETag))
			}
		}
		return nil
	}
	m["multipart_too_small"] = func(ctx context.Context) error {
		k := "mp/small"
		id, _, parts, err := mpParts(ctx, k, []int{1024, 1024})
		defer abort(k, id)
		if err != nil {
			return err
		}
		_, err = c.CompleteMultipartUpload(ctx, &s3.CompleteMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id,
			MultipartUpload: &types.CompletedMultipartUpload{Parts: parts}})
		return expectErr("Complete", err, nil, "EntityTooSmall")
	}
	m["high_level_upload"] = func(ctx context.Context) error {
		data := randBytes(20 << 20)
		up := manager.NewUploader(c, func(u *manager.Uploader) { u.PartSize = 5 << 20 })
		if _, err := up.Upload(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("hl/obj"), Body: bytes.NewReader(data)}); err != nil {
			return err
		}
		buf := manager.NewWriteAtBuffer(make([]byte, 0, len(data)))
		dl := manager.NewDownloader(c, func(d *manager.Downloader) { d.PartSize = 5 << 20 })
		if _, err := dl.Download(ctx, buf, &s3.GetObjectInput{Bucket: &b, Key: aws.String("hl/obj")}); err != nil {
			return err
		}
		return eqBytes("download", buf.Bytes(), data)
	}
	pc := s3.NewPresignClient(c)
	m["presigned_get"] = func(ctx context.Context) error {
		data := []byte("presigned get body")
		if _, err := v2Put(ctx, c, b, "ps/get", data); err != nil {
			return err
		}
		req, err := pc.PresignGetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("ps/get")})
		if err != nil {
			return err
		}
		st, got, err := httpDo(ctx, req.Method, req.URL, nil, req.SignedHeader)
		if err != nil {
			return err
		}
		if st != 200 {
			return failf("HTTP %d: %s", st, firstLine(got))
		}
		return eqBytes("GET", got, data)
	}
	m["presigned_put"] = func(ctx context.Context) error {
		data := []byte("presigned put body")
		req, err := pc.PresignPutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("ps/put")})
		if err != nil {
			return err
		}
		st, body, err := httpDo(ctx, req.Method, req.URL, data, req.SignedHeader)
		if err != nil {
			return err
		}
		if st != 200 {
			return failf("HTTP %d: %s", st, firstLine(body))
		}
		got, _, err := v2Get(ctx, c, b, "ps/put")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	for id, alg := range checksumScenarios {
		m[id] = func(ctx context.Context) error {
			data := []byte("checksum payload for " + alg)
			k := "ck/" + alg
			want := checksumB64(alg, data)
			_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: &k, Body: bytes.NewReader(data),
				ChecksumAlgorithm: types.ChecksumAlgorithm(alg)})
			if err != nil {
				return err
			}
			out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &b, Key: &k, ChecksumMode: types.ChecksumModeEnabled})
			if err != nil {
				return err
			}
			got, err := io.ReadAll(out.Body) // SDK validates the checksum while reading
			out.Body.Close()
			if err != nil {
				return err
			}
			return firstErr(eqBytes("GET", got, data), eqStr("GET checksum "+alg, aws.ToString(v2Checksum(out, alg)), want))
		}
	}
	m["content_md5_bad"] = func(ctx context.Context) error {
		_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("badmd5"), Body: strings.NewReader("payload"),
			ContentMD5: aws.String(md5B64([]byte("something else")))})
		return expectErr("PUT", err, []int{400}, "BadDigest", "InvalidDigest")
	}
	m["bad_credentials"] = func(ctx context.Context) error {
		bad := newV2Client(env.Config, "wrong-secret-key")
		_, err := bad.ListBuckets(ctx, &s3.ListBucketsInput{})
		return expectErr("ListBuckets", err, []int{403}, "SignatureDoesNotMatch", "InvalidAccessKeyId")
	}
	m["delete_bucket_not_empty"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "nonempty", []byte("x")); err != nil {
			return err
		}
		_, err := c.DeleteBucket(ctx, &s3.DeleteBucketInput{Bucket: &b})
		return expectErr("DeleteBucket", err, []int{409}, "BucketNotEmpty")
	}
	m["versioning"] = func(ctx context.Context) error {
		vb := b + "-v"
		if _, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: &vb}); err != nil {
			return err
		}
		if _, err := c.PutBucketVersioning(ctx, &s3.PutBucketVersioningInput{Bucket: &vb,
			VersioningConfiguration: &types.VersioningConfiguration{Status: types.BucketVersioningStatusEnabled}}); err != nil {
			return err
		}
		p1, err := v2Put(ctx, c, vb, "doc", []byte("version one"))
		if err != nil {
			return err
		}
		if _, err := v2Put(ctx, c, vb, "doc", []byte("version two")); err != nil {
			return err
		}
		lv, err := c.ListObjectVersions(ctx, &s3.ListObjectVersionsInput{Bucket: &vb, Prefix: aws.String("doc")})
		if err != nil {
			return err
		}
		if len(lv.Versions) != 2 {
			return failf("ListObjectVersions returned %d versions, want 2", len(lv.Versions))
		}
		v1 := aws.ToString(p1.VersionId)
		if v1 == "" || v1 == "null" {
			return failf("first PUT returned no VersionId")
		}
		found := false
		for _, v := range lv.Versions {
			found = found || aws.ToString(v.VersionId) == v1
		}
		if !found || aws.ToString(lv.Versions[0].VersionId) == aws.ToString(lv.Versions[1].VersionId) {
			return failf("version ids not distinct or first version missing")
		}
		out, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: &vb, Key: aws.String("doc"), VersionId: &v1})
		if err != nil {
			return err
		}
		got, _ := io.ReadAll(out.Body)
		out.Body.Close()
		if err := eqBytes("GET old version", got, []byte("version one")); err != nil {
			return err
		}
		latest, _, err := v2Get(ctx, c, vb, "doc")
		if err != nil {
			return err
		}
		return eqBytes("GET latest", latest, []byte("version two"))
	}
	m["object_tagging"] = func(ctx context.Context) error {
		if _, err := v2Put(ctx, c, b, "tagged", []byte("t")); err != nil {
			return err
		}
		_, err := c.PutObjectTagging(ctx, &s3.PutObjectTaggingInput{Bucket: &b, Key: aws.String("tagged"),
			Tagging: &types.Tagging{TagSet: []types.Tag{{Key: aws.String("env"), Value: aws.String("test")}, {Key: aws.String("team"), Value: aws.String("s3 compat")}}}})
		if err != nil {
			return err
		}
		out, err := c.GetObjectTagging(ctx, &s3.GetObjectTaggingInput{Bucket: &b, Key: aws.String("tagged")})
		if err != nil {
			return err
		}
		got := map[string]string{}
		for _, t := range out.TagSet {
			got[aws.ToString(t.Key)] = aws.ToString(t.Value)
		}
		return eqStr("tags", fmt.Sprint(got), "map[env:test team:s3 compat]")
	}
	m["delete_bucket"] = func(ctx context.Context) error {
		if err := adminEmptyBucket(ctx, c, b); err != nil {
			return fmt.Errorf("emptying bucket: %w", err)
		}
		if _, err := c.DeleteBucket(ctx, &s3.DeleteBucketInput{Bucket: &b}); err != nil {
			return err
		}
		_, err := c.HeadBucket(ctx, &s3.HeadBucketInput{Bucket: &b})
		return expectErr("HeadBucket after delete", err, []int{404}, "NotFound", "NoSuchBucket")
	}
	return m
}

func firstLine(b []byte) string {
	s := string(b)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	if len(s) > 200 {
		s = s[:200]
	}
	return s
}

var _ = errors.New
var _ = url.QueryEscape
