package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"slices"
	"strings"
	"time"

	"github.com/aws/aws-sdk-go/aws"
	"github.com/aws/aws-sdk-go/aws/credentials"
	"github.com/aws/aws-sdk-go/aws/session"
	"github.com/aws/aws-sdk-go/service/s3"
	"github.com/aws/aws-sdk-go/service/s3/s3manager"
)

// ---------- aws-sdk-go (v1, legacy) ----------

type V1SDK struct{}

func (V1SDK) Name() string   { return "aws-sdk-go" }
func (V1SDK) Module() string { return "github.com/aws/aws-sdk-go" }

func newV1Client(cfg Config, secret string) *s3.S3 {
	sess := session.Must(session.NewSession(&aws.Config{
		Endpoint:         aws.String(cfg.Endpoint),
		Region:           aws.String(cfg.Region),
		Credentials:      credentials.NewStaticCredentials(cfg.AccessKey, secret, ""),
		S3ForcePathStyle: aws.Bool(true),
		// Without this v1 path-cleans keys like "a//b" into "a/b" client-side.
		DisableRestProtocolURICleaning: aws.Bool(true),
	}))
	return s3.New(sess)
}

// lowerKeys: v1 and minio-go canonicalise user-metadata map keys (Color, X-Y)
// client-side, irrespective of what the server sent; compare case-insensitively.
func lowerKeys(m map[string]*string) map[string]string {
	out := map[string]string{}
	for k, v := range m {
		out[strings.ToLower(k)] = aws.StringValue(v)
	}
	return out
}

func (V1SDK) Scenarios(env *Env) map[string]func(ctx context.Context) error {
	c := newV1Client(env.Config, env.SecretKey)
	b := env.Bucket
	m := map[string]func(ctx context.Context) error{}

	put := func(ctx context.Context, bucket, key string, data []byte) (*s3.PutObjectOutput, error) {
		return c.PutObjectWithContext(ctx, &s3.PutObjectInput{Bucket: &bucket, Key: &key, Body: bytes.NewReader(data)})
	}
	get := func(ctx context.Context, bucket, key string) ([]byte, *s3.GetObjectOutput, error) {
		out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &bucket, Key: &key})
		if err != nil {
			return nil, nil, err
		}
		defer out.Body.Close()
		data, err := io.ReadAll(out.Body)
		return data, out, err
	}
	listAll := func(ctx context.Context, bucket, prefix string) ([]string, error) {
		var keys []string
		err := c.ListObjectsV2PagesWithContext(ctx, &s3.ListObjectsV2Input{Bucket: &bucket, Prefix: &prefix},
			func(p *s3.ListObjectsV2Output, last bool) bool {
				for _, o := range p.Contents {
					keys = append(keys, aws.StringValue(o.Key))
				}
				return true
			})
		return keys, err
	}

	m["create_bucket"] = func(ctx context.Context) error {
		_, err := c.CreateBucketWithContext(ctx, &s3.CreateBucketInput{Bucket: &b})
		return err
	}
	m["head_bucket"] = func(ctx context.Context) error {
		_, err := c.HeadBucketWithContext(ctx, &s3.HeadBucketInput{Bucket: &b})
		return err
	}
	m["head_bucket_missing"] = func(ctx context.Context) error {
		_, err := c.HeadBucketWithContext(ctx, &s3.HeadBucketInput{Bucket: aws.String("m-missing-" + randHex(6))})
		return expectErr("HeadBucket", err, []int{404}, "NotFound", "NoSuchBucket")
	}
	m["list_buckets"] = func(ctx context.Context) error {
		out, err := c.ListBucketsWithContext(ctx, &s3.ListBucketsInput{})
		if err != nil {
			return err
		}
		for _, bk := range out.Buckets {
			if aws.StringValue(bk.Name) == b {
				return nil
			}
		}
		return failf("bucket %s not in ListBuckets (%d buckets)", b, len(out.Buckets))
	}
	m["put_get_small"] = func(ctx context.Context) error {
		data := []byte("hello world")
		po, err := put(ctx, b, "small.txt", data)
		if err != nil {
			return err
		}
		got, gout, err := get(ctx, b, "small.txt")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data),
			eqStr("PUT ETag", unquote(aws.StringValue(po.ETag)), md5Hex(data)),
			eqStr("GET ETag", unquote(aws.StringValue(gout.ETag)), md5Hex(data)))
	}
	m["put_get_empty"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "empty", nil); err != nil {
			return err
		}
		got, out, err := get(ctx, b, "empty")
		if err != nil {
			return err
		}
		if len(got) != 0 || aws.Int64Value(out.ContentLength) != 0 {
			return failf("expected empty body, got %d bytes, Content-Length %d", len(got), aws.Int64Value(out.ContentLength))
		}
		return nil
	}
	m["put_get_large"] = func(ctx context.Context) error {
		data := randBytes(8 << 20)
		if _, err := put(ctx, b, "large.bin", data); err != nil {
			return err
		}
		got, _, err := get(ctx, b, "large.bin")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	m["special_keys"] = func(ctx context.Context) error {
		for _, k := range specialKeys {
			data := []byte("key:" + k)
			if _, err := put(ctx, b, k, data); err != nil {
				return fmt.Errorf("PUT %q: %w", k, err)
			}
			got, _, err := get(ctx, b, k)
			if err != nil {
				return fmt.Errorf("GET %q: %w", k, err)
			}
			if err := eqBytes("GET "+k, got, data); err != nil {
				return err
			}
		}
		keys, err := listAll(ctx, b, "")
		if err != nil {
			return err
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
		_, err := c.PutObjectWithContext(ctx, &s3.PutObjectInput{Bucket: &b, Key: &k, Body: strings.NewReader("meta"),
			ContentType: aws.String(mdContentType), CacheControl: aws.String(mdCacheControl),
			ContentDisposition: aws.String(mdContentDisposition), ContentEncoding: aws.String(mdContentEncoding),
			Metadata: aws.StringMap(metaIn)})
		if err != nil {
			return err
		}
		h, err := c.HeadObjectWithContext(ctx, &s3.HeadObjectInput{Bucket: &b, Key: &k})
		if err != nil {
			return err
		}
		return firstErr(
			eqStr("Content-Type", aws.StringValue(h.ContentType), mdContentType),
			eqStr("Cache-Control", aws.StringValue(h.CacheControl), mdCacheControl),
			eqStr("Content-Disposition", aws.StringValue(h.ContentDisposition), mdContentDisposition),
			eqStr("Content-Encoding", aws.StringValue(h.ContentEncoding), mdContentEncoding),
			checkMeta(lowerKeys(h.Metadata)))
	}
	m["get_range"] = func(ctx context.Context) error {
		data := randBytes(100)
		if _, err := put(ctx, b, "range.bin", data); err != nil {
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
			out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("range.bin"), Range: aws.String(tc.rng)})
			if err != nil {
				return fmt.Errorf("%s: %w", tc.rng, err)
			}
			got, err := io.ReadAll(out.Body)
			out.Body.Close()
			if err != nil {
				return err
			}
			if err := firstErr(eqBytes(tc.rng, got, tc.want), eqStr(tc.rng+" Content-Range", aws.StringValue(out.ContentRange), tc.cr)); err != nil {
				return err
			}
		}
		return nil
	}
	m["get_range_invalid"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "range-inv.bin", randBytes(100)); err != nil {
			return err
		}
		out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("range-inv.bin"), Range: aws.String("bytes=200-300")})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET bytes=200-300", err, []int{416}, "InvalidRange")
	}
	m["get_missing_key"] = func(ctx context.Context) error {
		_, _, err := get(ctx, b, "missing-"+randHex(4))
		return expectErr("GET", err, []int{404}, "NoSuchKey")
	}
	m["get_if_none_match"] = func(ctx context.Context) error {
		po, err := put(ctx, b, "cond.txt", []byte("conditional"))
		if err != nil {
			return err
		}
		out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("cond.txt"), IfNoneMatch: po.ETag})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET If-None-Match", err, []int{304}, "NotModified")
	}
	m["get_if_match_fail"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "cond2.txt", []byte("conditional")); err != nil {
			return err
		}
		out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &b, Key: aws.String("cond2.txt"), IfMatch: aws.String(`"wrong"`)})
		if err == nil {
			out.Body.Close()
		}
		return expectErr("GET If-Match", err, []int{412}, "PreconditionFailed")
	}
	m["put_if_none_match"] = func(ctx context.Context) error {
		// v1's PutObjectInput predates conditional writes (no IfNoneMatch field);
		// set the header through the request API instead.
		k := "ifnm-" + randHex(4)
		doPut := func() error {
			req, _ := c.PutObjectRequest(&s3.PutObjectInput{Bucket: &b, Key: &k, Body: strings.NewReader("x")})
			req.SetContext(ctx)
			req.HTTPRequest.Header.Set("If-None-Match", "*")
			return req.Send()
		}
		if err := doPut(); err != nil {
			return fmt.Errorf("first PUT (new key): %w", err)
		}
		return expectErr("second PUT", doPut(), []int{412}, "PreconditionFailed")
	}
	m["delete_object"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "del.txt", []byte("x")); err != nil {
			return err
		}
		if _, err := c.DeleteObjectWithContext(ctx, &s3.DeleteObjectInput{Bucket: &b, Key: aws.String("del.txt")}); err != nil {
			return err
		}
		_, err := c.HeadObjectWithContext(ctx, &s3.HeadObjectInput{Bucket: &b, Key: aws.String("del.txt")})
		return expectErr("HEAD after delete", err, []int{404}, "NotFound", "NoSuchKey")
	}
	m["delete_missing_object"] = func(ctx context.Context) error {
		_, err := c.DeleteObjectWithContext(ctx, &s3.DeleteObjectInput{Bucket: &b, Key: aws.String("never-" + randHex(4))})
		return err
	}
	m["delete_objects"] = func(ctx context.Context) error {
		keys := []string{"bulk/1", "bulk/2", "bulk/3"}
		var ids []*s3.ObjectIdentifier
		for _, k := range keys {
			if _, err := put(ctx, b, k, []byte(k)); err != nil {
				return err
			}
			ids = append(ids, &s3.ObjectIdentifier{Key: aws.String(k)})
		}
		out, err := c.DeleteObjectsWithContext(ctx, &s3.DeleteObjectsInput{Bucket: &b, Delete: &s3.Delete{Objects: ids}})
		if err != nil {
			return err
		}
		if len(out.Errors) > 0 {
			return failf("DeleteObjects errors: %s %s", aws.StringValue(out.Errors[0].Code), aws.StringValue(out.Errors[0].Message))
		}
		if len(out.Deleted) != 3 {
			return failf("Deleted has %d entries, want 3", len(out.Deleted))
		}
		left, err := listAll(ctx, b, "bulk/")
		if err != nil {
			return err
		}
		if len(left) != 0 {
			return failf("%d objects remain", len(left))
		}
		return nil
	}
	m["list_v2_basic"] = func(ctx context.Context) error {
		keys := []string{"lb/a", "lb/b", "lb/c"}
		datas := [][]byte{randBytes(1), randBytes(22), randBytes(333)}
		for i := len(keys) - 1; i >= 0; i-- {
			if _, err := put(ctx, b, keys[i], datas[i]); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2WithContext(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("lb/")})
		if err != nil {
			return err
		}
		if len(out.Contents) != 3 {
			return failf("got %d objects, want 3", len(out.Contents))
		}
		for i, o := range out.Contents {
			if err := firstErr(eqStr("key", aws.StringValue(o.Key), keys[i]),
				eqStr("etag "+keys[i], unquote(aws.StringValue(o.ETag)), md5Hex(datas[i]))); err != nil {
				return err
			}
			if aws.Int64Value(o.Size) != int64(len(datas[i])) {
				return failf("size %s: got %d want %d", keys[i], aws.Int64Value(o.Size), len(datas[i]))
			}
		}
		return nil
	}
	m["list_v2_prefix_delimiter"] = func(ctx context.Context) error {
		db := b + "-d"
		if _, err := c.CreateBucketWithContext(ctx, &s3.CreateBucketInput{Bucket: &db}); err != nil {
			return err
		}
		for _, k := range []string{"a/1", "a/2", "b/1", "c"} {
			if _, err := put(ctx, db, k, []byte(k)); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2WithContext(ctx, &s3.ListObjectsV2Input{Bucket: &db, Prefix: aws.String(""), Delimiter: aws.String("/")})
		if err != nil {
			return err
		}
		var cps, cs []string
		for _, p := range out.CommonPrefixes {
			cps = append(cps, aws.StringValue(p.Prefix))
		}
		for _, o := range out.Contents {
			cs = append(cs, aws.StringValue(o.Key))
		}
		return firstErr(eqStr("CommonPrefixes", fmt.Sprint(cps), "[a/ b/]"), eqStr("Contents", fmt.Sprint(cs), "[c]"))
	}
	m["list_v2_pagination"] = func(ctx context.Context) error {
		for _, k := range paginationKeys("pg/") {
			if _, err := put(ctx, b, k, []byte("p")); err != nil {
				return err
			}
		}
		var all []string
		pages := 0
		err := c.ListObjectsV2PagesWithContext(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("pg/"), MaxKeys: aws.Int64(10)},
			func(p *s3.ListObjectsV2Output, last bool) bool {
				pages++
				for _, o := range p.Contents {
					all = append(all, aws.StringValue(o.Key))
				}
				return pages < 10
			})
		if err != nil {
			return err
		}
		if pages != 3 || len(all) != 25 {
			return failf("got %d pages / %d keys, want 3 / 25", pages, len(all))
		}
		return sortedUnique(all)
	}
	m["list_v2_start_after"] = func(ctx context.Context) error {
		for _, k := range []string{"sa/0", "sa/1", "sa/2", "sa/3", "sa/4"} {
			if _, err := put(ctx, b, k, []byte("s")); err != nil {
				return err
			}
		}
		out, err := c.ListObjectsV2WithContext(ctx, &s3.ListObjectsV2Input{Bucket: &b, Prefix: aws.String("sa/"), StartAfter: aws.String("sa/2")})
		if err != nil {
			return err
		}
		var got []string
		for _, o := range out.Contents {
			got = append(got, aws.StringValue(o.Key))
		}
		return eqStr("keys", fmt.Sprint(got), "[sa/3 sa/4]")
	}
	m["list_v1"] = func(ctx context.Context) error {
		for _, k := range []string{"l1/0", "l1/1", "l1/2", "l1/3", "l1/4"} {
			if _, err := put(ctx, b, k, []byte("v1")); err != nil {
				return err
			}
		}
		var got []string
		marker := aws.String("l1/0")
		for i := 0; i < 10; i++ {
			out, err := c.ListObjectsWithContext(ctx, &s3.ListObjectsInput{Bucket: &b, Prefix: aws.String("l1/"), Marker: marker, MaxKeys: aws.Int64(2)})
			if err != nil {
				return err
			}
			for _, o := range out.Contents {
				got = append(got, aws.StringValue(o.Key))
			}
			if !aws.BoolValue(out.IsTruncated) {
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
		_, err := c.PutObjectWithContext(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("cp/src"), Body: bytes.NewReader(data),
			ContentType: aws.String("text/x-copy"), Metadata: aws.StringMap(metaIn)})
		if err != nil {
			return err
		}
		if _, err := c.CopyObjectWithContext(ctx, &s3.CopyObjectInput{Bucket: &b, Key: aws.String("cp/dst"), CopySource: aws.String(b + "/cp/src")}); err != nil {
			return err
		}
		got, out, err := get(ctx, b, "cp/dst")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("Content-Type", aws.StringValue(out.ContentType), "text/x-copy"), checkMeta(lowerKeys(out.Metadata)))
	}
	m["copy_object_replace_metadata"] = func(ctx context.Context) error {
		_, err := c.PutObjectWithContext(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("cpr/src"), Body: strings.NewReader("x"),
			ContentType: aws.String("text/x-old"), Metadata: aws.StringMap(metaIn)})
		if err != nil {
			return err
		}
		_, err = c.CopyObjectWithContext(ctx, &s3.CopyObjectInput{Bucket: &b, Key: aws.String("cpr/dst"), CopySource: aws.String(b + "/cpr/src"),
			MetadataDirective: aws.String(s3.MetadataDirectiveReplace), ContentType: aws.String("text/x-new"),
			Metadata: aws.StringMap(map[string]string{"color": "Red", "fresh": "yes"})})
		if err != nil {
			return err
		}
		h, err := c.HeadObjectWithContext(ctx, &s3.HeadObjectInput{Bucket: &b, Key: aws.String("cpr/dst")})
		if err != nil {
			return err
		}
		return firstErr(eqStr("Content-Type", aws.StringValue(h.ContentType), "text/x-new"),
			eqStr("metadata", fmt.Sprint(lowerKeys(h.Metadata)), "map[color:Red fresh:yes]"))
	}
	mpParts := func(ctx context.Context, key string, sizes []int) (string, [][]byte, []*s3.CompletedPart, error) {
		cr, err := c.CreateMultipartUploadWithContext(ctx, &s3.CreateMultipartUploadInput{Bucket: &b, Key: &key})
		if err != nil {
			return "", nil, nil, err
		}
		var datas [][]byte
		var parts []*s3.CompletedPart
		for i, sz := range sizes {
			d := randBytes(sz)
			up, err := c.UploadPartWithContext(ctx, &s3.UploadPartInput{Bucket: &b, Key: &key, UploadId: cr.UploadId,
				PartNumber: aws.Int64(int64(i + 1)), Body: bytes.NewReader(d)})
			if err != nil {
				return *cr.UploadId, nil, nil, err
			}
			datas = append(datas, d)
			parts = append(parts, &s3.CompletedPart{ETag: up.ETag, PartNumber: aws.Int64(int64(i + 1))})
		}
		return *cr.UploadId, datas, parts, nil
	}
	abort := func(key, id string) {
		if id != "" {
			_, _ = c.AbortMultipartUpload(&s3.AbortMultipartUploadInput{Bucket: &b, Key: &key, UploadId: &id})
		}
	}
	m["multipart_upload"] = func(ctx context.Context) error {
		k := "mp/obj"
		id, datas, parts, err := mpParts(ctx, k, []int{5 << 20, 5 << 20, 1024})
		if err != nil {
			abort(k, id)
			return err
		}
		out, err := c.CompleteMultipartUploadWithContext(ctx, &s3.CompleteMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id,
			MultipartUpload: &s3.CompletedMultipartUpload{Parts: parts}})
		if err != nil {
			abort(k, id)
			return err
		}
		got, _, err := get(ctx, b, k)
		if err != nil {
			return err
		}
		if err := eqBytes("GET", got, bytes.Join(datas, nil)); err != nil {
			return err
		}
		if !isMultipartETag(aws.StringValue(out.ETag), 3) {
			return failf("multipart ETag %q not of form <hex>-3", aws.StringValue(out.ETag))
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
		if _, err := c.AbortMultipartUploadWithContext(ctx, &s3.AbortMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id}); err != nil {
			return err
		}
		_, err = c.ListPartsWithContext(ctx, &s3.ListPartsInput{Bucket: &b, Key: &k, UploadId: &id})
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
		out, err := c.ListPartsWithContext(ctx, &s3.ListPartsInput{Bucket: &b, Key: &k, UploadId: &id})
		if err != nil {
			return err
		}
		if len(out.Parts) != 2 {
			return failf("ListParts returned %d parts, want 2", len(out.Parts))
		}
		for i, p := range out.Parts {
			if aws.Int64Value(p.PartNumber) != int64(i+1) || aws.Int64Value(p.Size) != int64(sizes[i]) ||
				unquote(aws.StringValue(p.ETag)) != unquote(aws.StringValue(parts[i].ETag)) || unquote(aws.StringValue(p.ETag)) != md5Hex(datas[i]) {
				return failf("part %d: number=%d size=%d etag=%s", i+1, aws.Int64Value(p.PartNumber), aws.Int64Value(p.Size), aws.StringValue(p.ETag))
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
		_, err = c.CompleteMultipartUploadWithContext(ctx, &s3.CompleteMultipartUploadInput{Bucket: &b, Key: &k, UploadId: &id,
			MultipartUpload: &s3.CompletedMultipartUpload{Parts: parts}})
		return expectErr("Complete", err, nil, "EntityTooSmall")
	}
	m["high_level_upload"] = func(ctx context.Context) error {
		data := randBytes(20 << 20)
		up := s3manager.NewUploaderWithClient(c, func(u *s3manager.Uploader) { u.PartSize = 5 << 20 })
		if _, err := up.UploadWithContext(ctx, &s3manager.UploadInput{Bucket: &b, Key: aws.String("hl/obj"), Body: bytes.NewReader(data)}); err != nil {
			return err
		}
		buf := aws.NewWriteAtBuffer(make([]byte, 0, len(data)))
		dl := s3manager.NewDownloaderWithClient(c, func(d *s3manager.Downloader) { d.PartSize = 5 << 20 })
		if _, err := dl.DownloadWithContext(ctx, buf, &s3.GetObjectInput{Bucket: &b, Key: aws.String("hl/obj")}); err != nil {
			return err
		}
		return eqBytes("download", buf.Bytes(), data)
	}
	m["presigned_get"] = func(ctx context.Context) error {
		data := []byte("presigned get body")
		if _, err := put(ctx, b, "ps/get", data); err != nil {
			return err
		}
		req, _ := c.GetObjectRequest(&s3.GetObjectInput{Bucket: &b, Key: aws.String("ps/get")})
		u, err := req.Presign(15 * time.Minute)
		if err != nil {
			return err
		}
		st, got, err := httpDo(ctx, http.MethodGet, u, nil, nil)
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
		req, _ := c.PutObjectRequest(&s3.PutObjectInput{Bucket: &b, Key: aws.String("ps/put")})
		u, err := req.Presign(15 * time.Minute)
		if err != nil {
			return err
		}
		st, body, err := httpDo(ctx, http.MethodPut, u, data, nil)
		if err != nil {
			return err
		}
		if st != 200 {
			return failf("HTTP %d: %s", st, firstLine(body))
		}
		got, _, err := get(ctx, b, "ps/put")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	for id, alg := range checksumScenarios {
		m[id] = func(ctx context.Context) error {
			if alg == "CRC64NVME" {
				return Skip("aws-sdk-go v1 has no CRC64NVME checksum support (no ChecksumCRC64NVME field)")
			}
			// v1 has the ChecksumAlgorithm/Checksum* fields but does not compute
			// flexible checksums itself, so the value is supplied by the caller.
			data := []byte("checksum payload for " + alg)
			k := "ck/" + alg
			want := checksumB64(alg, data)
			in := &s3.PutObjectInput{Bucket: &b, Key: &k, Body: bytes.NewReader(data), ChecksumAlgorithm: aws.String(alg)}
			switch alg {
			case "CRC32":
				in.ChecksumCRC32 = &want
			case "CRC32C":
				in.ChecksumCRC32C = &want
			case "SHA1":
				in.ChecksumSHA1 = &want
			case "SHA256":
				in.ChecksumSHA256 = &want
			}
			if _, err := c.PutObjectWithContext(ctx, in); err != nil {
				return err
			}
			out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &b, Key: &k, ChecksumMode: aws.String(s3.ChecksumModeEnabled)})
			if err != nil {
				return err
			}
			got, err := io.ReadAll(out.Body)
			out.Body.Close()
			if err != nil {
				return err
			}
			var gotSum *string
			switch alg {
			case "CRC32":
				gotSum = out.ChecksumCRC32
			case "CRC32C":
				gotSum = out.ChecksumCRC32C
			case "SHA1":
				gotSum = out.ChecksumSHA1
			case "SHA256":
				gotSum = out.ChecksumSHA256
			}
			return firstErr(eqBytes("GET", got, data), eqStr("GET checksum "+alg, aws.StringValue(gotSum), want))
		}
	}
	m["content_md5_bad"] = func(ctx context.Context) error {
		_, err := c.PutObjectWithContext(ctx, &s3.PutObjectInput{Bucket: &b, Key: aws.String("badmd5"), Body: strings.NewReader("payload"),
			ContentMD5: aws.String(md5B64([]byte("something else")))})
		return expectErr("PUT", err, []int{400}, "BadDigest", "InvalidDigest")
	}
	m["bad_credentials"] = func(ctx context.Context) error {
		bad := newV1Client(env.Config, "wrong-secret-key")
		_, err := bad.ListBucketsWithContext(ctx, &s3.ListBucketsInput{})
		return expectErr("ListBuckets", err, []int{403}, "SignatureDoesNotMatch", "InvalidAccessKeyId")
	}
	m["delete_bucket_not_empty"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "nonempty", []byte("x")); err != nil {
			return err
		}
		_, err := c.DeleteBucketWithContext(ctx, &s3.DeleteBucketInput{Bucket: &b})
		return expectErr("DeleteBucket", err, []int{409}, "BucketNotEmpty")
	}
	m["versioning"] = func(ctx context.Context) error {
		vb := b + "-v"
		if _, err := c.CreateBucketWithContext(ctx, &s3.CreateBucketInput{Bucket: &vb}); err != nil {
			return err
		}
		if _, err := c.PutBucketVersioningWithContext(ctx, &s3.PutBucketVersioningInput{Bucket: &vb,
			VersioningConfiguration: &s3.VersioningConfiguration{Status: aws.String(s3.BucketVersioningStatusEnabled)}}); err != nil {
			return err
		}
		p1, err := put(ctx, vb, "doc", []byte("version one"))
		if err != nil {
			return err
		}
		if _, err := put(ctx, vb, "doc", []byte("version two")); err != nil {
			return err
		}
		lv, err := c.ListObjectVersionsWithContext(ctx, &s3.ListObjectVersionsInput{Bucket: &vb, Prefix: aws.String("doc")})
		if err != nil {
			return err
		}
		if len(lv.Versions) != 2 {
			return failf("ListObjectVersions returned %d versions, want 2", len(lv.Versions))
		}
		v1 := aws.StringValue(p1.VersionId)
		if v1 == "" || v1 == "null" {
			return failf("first PUT returned no VersionId")
		}
		found := false
		for _, v := range lv.Versions {
			found = found || aws.StringValue(v.VersionId) == v1
		}
		if !found || aws.StringValue(lv.Versions[0].VersionId) == aws.StringValue(lv.Versions[1].VersionId) {
			return failf("version ids not distinct or first version missing")
		}
		out, err := c.GetObjectWithContext(ctx, &s3.GetObjectInput{Bucket: &vb, Key: aws.String("doc"), VersionId: &v1})
		if err != nil {
			return err
		}
		got, _ := io.ReadAll(out.Body)
		out.Body.Close()
		if err := eqBytes("GET old version", got, []byte("version one")); err != nil {
			return err
		}
		latest, _, err := get(ctx, vb, "doc")
		if err != nil {
			return err
		}
		return eqBytes("GET latest", latest, []byte("version two"))
	}
	m["object_tagging"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "tagged", []byte("t")); err != nil {
			return err
		}
		_, err := c.PutObjectTaggingWithContext(ctx, &s3.PutObjectTaggingInput{Bucket: &b, Key: aws.String("tagged"),
			Tagging: &s3.Tagging{TagSet: []*s3.Tag{{Key: aws.String("env"), Value: aws.String("test")}, {Key: aws.String("team"), Value: aws.String("s3 compat")}}}})
		if err != nil {
			return err
		}
		out, err := c.GetObjectTaggingWithContext(ctx, &s3.GetObjectTaggingInput{Bucket: &b, Key: aws.String("tagged")})
		if err != nil {
			return err
		}
		got := map[string]string{}
		for _, t := range out.TagSet {
			got[aws.StringValue(t.Key)] = aws.StringValue(t.Value)
		}
		return eqStr("tags", fmt.Sprint(got), "map[env:test team:s3 compat]")
	}
	m["delete_bucket"] = func(ctx context.Context) error {
		// Empty with the SDK's own batch-delete iterator.
		iter := s3manager.NewDeleteListIterator(c, &s3.ListObjectsInput{Bucket: &b})
		if err := s3manager.NewBatchDeleteWithClient(c).Delete(ctx, iter); err != nil {
			return fmt.Errorf("emptying bucket: %w", err)
		}
		if mu, err := c.ListMultipartUploadsWithContext(ctx, &s3.ListMultipartUploadsInput{Bucket: &b}); err == nil {
			for _, u := range mu.Uploads {
				_, _ = c.AbortMultipartUploadWithContext(ctx, &s3.AbortMultipartUploadInput{Bucket: &b, Key: u.Key, UploadId: u.UploadId})
			}
		}
		if _, err := c.DeleteBucketWithContext(ctx, &s3.DeleteBucketInput{Bucket: &b}); err != nil {
			return err
		}
		_, err := c.HeadBucketWithContext(ctx, &s3.HeadBucketInput{Bucket: &b})
		return expectErr("HeadBucket after delete", err, []int{404}, "NotFound", "NoSuchBucket")
	}
	return m
}
