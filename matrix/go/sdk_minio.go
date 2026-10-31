package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"maps"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"time"

	"github.com/minio/minio-go/v7"
	"github.com/minio/minio-go/v7/pkg/credentials"
	"github.com/minio/minio-go/v7/pkg/tags"
)

// ---------- minio-go v7 ----------

type MinioSDK struct{}

func (MinioSDK) Name() string   { return "minio-go" }
func (MinioSDK) Module() string { return "github.com/minio/minio-go/v7" }

func newMinioClient(cfg Config, secret string, trailing bool) *minio.Client {
	u, err := url.Parse(cfg.Endpoint)
	if err != nil {
		panic(err)
	}
	c, err := minio.New(u.Host, &minio.Options{
		Creds:        credentials.NewStaticV4(cfg.AccessKey, secret, ""),
		Secure:       u.Scheme == "https",
		Region:       cfg.Region,
		BucketLookup: minio.BucketLookupPath,
		// Required by minio-go for PutObjectOptions.Checksum (sent as a trailer);
		// only enabled for the checksum scenarios so the rest use SDK defaults.
		TrailingHeaders: trailing,
	})
	if err != nil {
		panic(err)
	}
	return c
}

func lowerMap(m map[string]string) map[string]string {
	out := map[string]string{}
	for k, v := range m {
		out[strings.ToLower(k)] = v
	}
	return out
}

func (MinioSDK) Scenarios(env *Env) map[string]func(ctx context.Context) error {
	c := newMinioClient(env.Config, env.SecretKey, false)
	ck := newMinioClient(env.Config, env.SecretKey, true)
	core := &minio.Core{Client: c}
	b := env.Bucket
	m := map[string]func(ctx context.Context) error{}

	put := func(ctx context.Context, bucket, key string, data []byte, opts minio.PutObjectOptions) (minio.UploadInfo, error) {
		return c.PutObject(ctx, bucket, key, bytes.NewReader(data), int64(len(data)), opts)
	}
	get := func(ctx context.Context, bucket, key string, opts minio.GetObjectOptions) ([]byte, minio.ObjectInfo, http.Header, error) {
		rc, info, hdr, err := core.GetObject(ctx, bucket, key, opts)
		if err != nil {
			return nil, info, hdr, err
		}
		defer rc.Close()
		data, err := io.ReadAll(rc)
		return data, info, hdr, err
	}
	listKeys := func(ctx context.Context, bucket string, opts minio.ListObjectsOptions) ([]minio.ObjectInfo, error) {
		var out []minio.ObjectInfo
		for o := range c.ListObjects(ctx, bucket, opts) {
			if o.Err != nil {
				return nil, o.Err
			}
			out = append(out, o)
		}
		return out, nil
	}
	keysOf := func(objs []minio.ObjectInfo) []string {
		var ks []string
		for _, o := range objs {
			ks = append(ks, o.Key)
		}
		return ks
	}

	m["create_bucket"] = func(ctx context.Context) error {
		return c.MakeBucket(ctx, b, minio.MakeBucketOptions{})
	}
	m["head_bucket"] = func(ctx context.Context) error {
		ok, err := c.BucketExists(ctx, b)
		if err != nil {
			return err
		}
		if !ok {
			return failf("BucketExists returned false")
		}
		return nil
	}
	m["head_bucket_missing"] = func(ctx context.Context) error {
		// BucketExists maps a 404/NoSuchBucket on HEAD to (false, nil).
		ok, err := c.BucketExists(ctx, "m-missing-"+randHex(6))
		if err != nil {
			return err
		}
		if ok {
			return failf("BucketExists returned true for a missing bucket")
		}
		return nil
	}
	m["list_buckets"] = func(ctx context.Context) error {
		bs, err := c.ListBuckets(ctx)
		if err != nil {
			return err
		}
		for _, bk := range bs {
			if bk.Name == b {
				return nil
			}
		}
		return failf("bucket %s not in ListBuckets (%d buckets)", b, len(bs))
	}
	m["put_get_small"] = func(ctx context.Context) error {
		data := []byte("hello world")
		ui, err := put(ctx, b, "small.txt", data, minio.PutObjectOptions{})
		if err != nil {
			return err
		}
		got, info, _, err := get(ctx, b, "small.txt", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("PUT ETag", unquote(ui.ETag), md5Hex(data)), eqStr("GET ETag", unquote(info.ETag), md5Hex(data)))
	}
	m["put_get_empty"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "empty", nil, minio.PutObjectOptions{}); err != nil {
			return err
		}
		got, _, hdr, err := get(ctx, b, "empty", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		if len(got) != 0 || hdr.Get("Content-Length") != "0" {
			return failf("expected empty body, got %d bytes, Content-Length %q", len(got), hdr.Get("Content-Length"))
		}
		return nil
	}
	m["put_get_large"] = func(ctx context.Context) error {
		data := randBytes(8 << 20)
		// DisableMultipart: the scenario requires a single PUT.
		if _, err := put(ctx, b, "large.bin", data, minio.PutObjectOptions{DisableMultipart: true}); err != nil {
			return err
		}
		got, _, _, err := get(ctx, b, "large.bin", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	m["special_keys"] = func(ctx context.Context) error {
		for _, k := range specialKeys {
			data := []byte("key:" + k)
			if _, err := put(ctx, b, k, data, minio.PutObjectOptions{}); err != nil {
				return fmt.Errorf("PUT %q: %w", k, err)
			}
			got, _, _, err := get(ctx, b, k, minio.GetObjectOptions{})
			if err != nil {
				return fmt.Errorf("GET %q: %w", k, err)
			}
			if err := eqBytes("GET "+k, got, data); err != nil {
				return err
			}
		}
		objs, err := listKeys(ctx, b, minio.ListObjectsOptions{Recursive: true})
		if err != nil {
			return err
		}
		keys := keysOf(objs)
		for _, k := range specialKeys {
			if !slices.Contains(keys, k) {
				return failf("key %q missing from list", k)
			}
		}
		return nil
	}
	m["head_object_metadata"] = func(ctx context.Context) error {
		_, err := put(ctx, b, "meta.txt", []byte("meta"), minio.PutObjectOptions{ContentType: mdContentType, CacheControl: mdCacheControl,
			ContentDisposition: mdContentDisposition, ContentEncoding: mdContentEncoding, UserMetadata: maps.Clone(metaIn)})
		if err != nil {
			return err
		}
		info, err := c.StatObject(ctx, b, "meta.txt", minio.StatObjectOptions{})
		if err != nil {
			return err
		}
		return firstErr(
			eqStr("Content-Type", info.ContentType, mdContentType),
			eqStr("Cache-Control", info.Metadata.Get("Cache-Control"), mdCacheControl),
			eqStr("Content-Disposition", info.Metadata.Get("Content-Disposition"), mdContentDisposition),
			eqStr("Content-Encoding", info.Metadata.Get("Content-Encoding"), mdContentEncoding),
			// minio-go canonicalises the map keys (Color, X-Y) client-side.
			checkMeta(lowerMap(info.UserMetadata)))
	}
	m["get_range"] = func(ctx context.Context) error {
		data := randBytes(100)
		if _, err := put(ctx, b, "range.bin", data, minio.PutObjectOptions{}); err != nil {
			return err
		}
		cases := []struct {
			start, end int64
			name, cr   string
			want       []byte
		}{
			{10, 19, "bytes=10-19", "bytes 10-19/100", data[10:20]},
			{0, -5, "bytes=-5", "bytes 95-99/100", data[95:]},
			{95, 0, "bytes=95-", "bytes 95-99/100", data[95:]},
		}
		for _, tc := range cases {
			var o minio.GetObjectOptions
			if err := o.SetRange(tc.start, tc.end); err != nil {
				return err
			}
			got, _, hdr, err := get(ctx, b, "range.bin", o)
			if err != nil {
				return fmt.Errorf("%s: %w", tc.name, err)
			}
			if err := firstErr(eqBytes(tc.name, got, tc.want), eqStr(tc.name+" Content-Range", hdr.Get("Content-Range"), tc.cr)); err != nil {
				return err
			}
		}
		return nil
	}
	m["get_range_invalid"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "range-inv.bin", randBytes(100), minio.PutObjectOptions{}); err != nil {
			return err
		}
		var o minio.GetObjectOptions
		_ = o.SetRange(200, 300)
		_, _, _, err := get(ctx, b, "range-inv.bin", o)
		return expectErr("GET bytes=200-300", err, []int{416}, "InvalidRange")
	}
	m["get_missing_key"] = func(ctx context.Context) error {
		_, _, _, err := get(ctx, b, "missing-"+randHex(4), minio.GetObjectOptions{})
		return expectErr("GET", err, []int{404}, "NoSuchKey")
	}
	m["get_if_none_match"] = func(ctx context.Context) error {
		ui, err := put(ctx, b, "cond.txt", []byte("conditional"), minio.PutObjectOptions{})
		if err != nil {
			return err
		}
		var o minio.GetObjectOptions
		_ = o.SetMatchETagExcept(ui.ETag)
		_, _, _, err = get(ctx, b, "cond.txt", o)
		return expectErr("GET If-None-Match", err, []int{304}, "NotModified")
	}
	m["get_if_match_fail"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "cond2.txt", []byte("conditional"), minio.PutObjectOptions{}); err != nil {
			return err
		}
		var o minio.GetObjectOptions
		_ = o.SetMatchETag("wrong")
		_, _, _, err := get(ctx, b, "cond2.txt", o)
		return expectErr("GET If-Match", err, []int{412}, "PreconditionFailed")
	}
	m["put_if_none_match"] = func(ctx context.Context) error {
		k := "ifnm-" + randHex(4)
		doPut := func() error {
			var o minio.PutObjectOptions
			o.SetMatchETagExcept("*")
			_, err := put(ctx, b, k, []byte("x"), o)
			return err
		}
		if err := doPut(); err != nil {
			return fmt.Errorf("first PUT (new key): %w", err)
		}
		return expectErr("second PUT", doPut(), []int{412}, "PreconditionFailed")
	}
	m["delete_object"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "del.txt", []byte("x"), minio.PutObjectOptions{}); err != nil {
			return err
		}
		if err := c.RemoveObject(ctx, b, "del.txt", minio.RemoveObjectOptions{}); err != nil {
			return err
		}
		_, err := c.StatObject(ctx, b, "del.txt", minio.StatObjectOptions{})
		return expectErr("HEAD after delete", err, []int{404}, "NotFound", "NoSuchKey")
	}
	m["delete_missing_object"] = func(ctx context.Context) error {
		return c.RemoveObject(ctx, b, "never-"+randHex(4), minio.RemoveObjectOptions{})
	}
	m["delete_objects"] = func(ctx context.Context) error {
		keys := []string{"bulk/1", "bulk/2", "bulk/3"}
		ch := make(chan minio.ObjectInfo, len(keys))
		for _, k := range keys {
			if _, err := put(ctx, b, k, []byte(k), minio.PutObjectOptions{}); err != nil {
				return err
			}
			ch <- minio.ObjectInfo{Key: k}
		}
		close(ch)
		for e := range c.RemoveObjects(ctx, b, ch, minio.RemoveObjectsOptions{}) {
			return fmt.Errorf("RemoveObjects %s: %w", e.ObjectName, e.Err)
		}
		left, err := listKeys(ctx, b, minio.ListObjectsOptions{Prefix: "bulk/", Recursive: true})
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
			if _, err := put(ctx, b, keys[i], datas[i], minio.PutObjectOptions{}); err != nil {
				return err
			}
		}
		objs, err := listKeys(ctx, b, minio.ListObjectsOptions{Prefix: "lb/", Recursive: true})
		if err != nil {
			return err
		}
		if len(objs) != 3 {
			return failf("got %d objects, want 3", len(objs))
		}
		for i, o := range objs {
			if err := firstErr(eqStr("key", o.Key, keys[i]), eqStr("etag "+keys[i], unquote(o.ETag), md5Hex(datas[i]))); err != nil {
				return err
			}
			if o.Size != int64(len(datas[i])) {
				return failf("size %s: got %d want %d", keys[i], o.Size, len(datas[i]))
			}
		}
		return nil
	}
	m["list_v2_prefix_delimiter"] = func(ctx context.Context) error {
		db := b + "-d"
		if err := c.MakeBucket(ctx, db, minio.MakeBucketOptions{}); err != nil {
			return err
		}
		for _, k := range []string{"a/1", "a/2", "b/1", "c"} {
			if _, err := put(ctx, db, k, []byte(k), minio.PutObjectOptions{}); err != nil {
				return err
			}
		}
		res, err := core.ListObjectsV2(db, "", "", "", "/", 1000)
		if err != nil {
			return err
		}
		var cps, cs []string
		for _, p := range res.CommonPrefixes {
			cps = append(cps, p.Prefix)
		}
		for _, o := range res.Contents {
			cs = append(cs, o.Key)
		}
		return firstErr(eqStr("CommonPrefixes", fmt.Sprint(cps), "[a/ b/]"), eqStr("Contents", fmt.Sprint(cs), "[c]"))
	}
	m["list_v2_pagination"] = func(ctx context.Context) error {
		for _, k := range paginationKeys("pg/") {
			if _, err := put(ctx, b, k, []byte("p"), minio.PutObjectOptions{}); err != nil {
				return err
			}
		}
		var all []string
		pages := 0
		token := ""
		for {
			res, err := core.ListObjectsV2(b, "pg/", "", token, "", 10)
			if err != nil {
				return err
			}
			pages++
			for _, o := range res.Contents {
				all = append(all, o.Key)
			}
			if !res.IsTruncated {
				break
			}
			if pages > 10 {
				return failf("too many pages")
			}
			token = res.NextContinuationToken
		}
		if pages != 3 || len(all) != 25 {
			return failf("got %d pages / %d keys, want 3 / 25", pages, len(all))
		}
		return sortedUnique(all)
	}
	m["list_v2_start_after"] = func(ctx context.Context) error {
		for _, k := range []string{"sa/0", "sa/1", "sa/2", "sa/3", "sa/4"} {
			if _, err := put(ctx, b, k, []byte("s"), minio.PutObjectOptions{}); err != nil {
				return err
			}
		}
		objs, err := listKeys(ctx, b, minio.ListObjectsOptions{Prefix: "sa/", StartAfter: "sa/2", Recursive: true})
		if err != nil {
			return err
		}
		return eqStr("keys", fmt.Sprint(keysOf(objs)), "[sa/3 sa/4]")
	}
	m["list_v1"] = func(ctx context.Context) error {
		for _, k := range []string{"l1/0", "l1/1", "l1/2", "l1/3", "l1/4"} {
			if _, err := put(ctx, b, k, []byte("v1"), minio.PutObjectOptions{}); err != nil {
				return err
			}
		}
		var got []string
		marker := "l1/0"
		for i := 0; i < 10; i++ {
			res, err := core.ListObjects(b, "l1/", marker, "", 2)
			if err != nil {
				return err
			}
			for _, o := range res.Contents {
				got = append(got, o.Key)
			}
			if !res.IsTruncated {
				break
			}
			if len(res.Contents) == 0 {
				return failf("truncated page with no contents")
			}
			marker = res.Contents[len(res.Contents)-1].Key
		}
		return eqStr("keys after marker", fmt.Sprint(got), "[l1/1 l1/2 l1/3 l1/4]")
	}
	m["copy_object"] = func(ctx context.Context) error {
		data := []byte("copy me")
		if _, err := put(ctx, b, "cp/src", data, minio.PutObjectOptions{ContentType: "text/x-copy", UserMetadata: maps.Clone(metaIn)}); err != nil {
			return err
		}
		if _, err := c.CopyObject(ctx, minio.CopyDestOptions{Bucket: b, Object: "cp/dst"}, minio.CopySrcOptions{Bucket: b, Object: "cp/src"}); err != nil {
			return err
		}
		got, info, _, err := get(ctx, b, "cp/dst", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("Content-Type", info.ContentType, "text/x-copy"), checkMeta(lowerMap(info.UserMetadata)))
	}
	m["copy_object_replace_metadata"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "cpr/src", []byte("x"), minio.PutObjectOptions{ContentType: "text/x-old", UserMetadata: maps.Clone(metaIn)}); err != nil {
			return err
		}
		_, err := c.CopyObject(ctx, minio.CopyDestOptions{Bucket: b, Object: "cpr/dst", ReplaceMetadata: true,
			ContentType: "text/x-new", UserMetadata: map[string]string{"color": "Red", "fresh": "yes"}},
			minio.CopySrcOptions{Bucket: b, Object: "cpr/src"})
		if err != nil {
			return err
		}
		info, err := c.StatObject(ctx, b, "cpr/dst", minio.StatObjectOptions{})
		if err != nil {
			return err
		}
		return firstErr(eqStr("Content-Type", info.ContentType, "text/x-new"),
			eqStr("metadata", fmt.Sprint(lowerMap(info.UserMetadata)), "map[color:Red fresh:yes]"))
	}
	mpParts := func(ctx context.Context, key string, sizes []int) (string, [][]byte, []minio.CompletePart, error) {
		id, err := core.NewMultipartUpload(ctx, b, key, minio.PutObjectOptions{})
		if err != nil {
			return "", nil, nil, err
		}
		var datas [][]byte
		var parts []minio.CompletePart
		for i, sz := range sizes {
			d := randBytes(sz)
			p, err := core.PutObjectPart(ctx, b, key, id, i+1, bytes.NewReader(d), int64(sz), minio.PutObjectPartOptions{})
			if err != nil {
				return id, nil, nil, err
			}
			datas = append(datas, d)
			parts = append(parts, minio.CompletePart{PartNumber: i + 1, ETag: p.ETag})
		}
		return id, datas, parts, nil
	}
	abort := func(key, id string) {
		if id != "" {
			_ = core.AbortMultipartUpload(context.Background(), b, key, id)
		}
	}
	m["multipart_upload"] = func(ctx context.Context) error {
		k := "mp/obj"
		id, datas, parts, err := mpParts(ctx, k, []int{5 << 20, 5 << 20, 1024})
		if err != nil {
			abort(k, id)
			return err
		}
		ui, err := core.CompleteMultipartUpload(ctx, b, k, id, parts, minio.PutObjectOptions{})
		if err != nil {
			abort(k, id)
			return err
		}
		got, _, _, err := get(ctx, b, k, minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		if err := eqBytes("GET", got, bytes.Join(datas, nil)); err != nil {
			return err
		}
		if !isMultipartETag(ui.ETag, 3) {
			return failf("multipart ETag %q not of form <hex>-3", ui.ETag)
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
		if err := core.AbortMultipartUpload(ctx, b, k, id); err != nil {
			return err
		}
		_, err = core.ListObjectParts(ctx, b, k, id, 0, 1000)
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
		res, err := core.ListObjectParts(ctx, b, k, id, 0, 1000)
		if err != nil {
			return err
		}
		if len(res.ObjectParts) != 2 {
			return failf("ListParts returned %d parts, want 2", len(res.ObjectParts))
		}
		for i, p := range res.ObjectParts {
			if p.PartNumber != i+1 || p.Size != int64(sizes[i]) || unquote(p.ETag) != unquote(parts[i].ETag) || unquote(p.ETag) != md5Hex(datas[i]) {
				return failf("part %d: number=%d size=%d etag=%s", i+1, p.PartNumber, p.Size, p.ETag)
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
		_, err = core.CompleteMultipartUpload(ctx, b, k, id, parts, minio.PutObjectOptions{})
		return expectErr("Complete", err, nil, "EntityTooSmall")
	}
	m["high_level_upload"] = func(ctx context.Context) error {
		// PutObject is minio-go's high-level transfer API: it splits into parts automatically.
		data := randBytes(20 << 20)
		ui, err := c.PutObject(ctx, b, "hl/obj", bytes.NewReader(data), int64(len(data)), minio.PutObjectOptions{PartSize: 5 << 20})
		if err != nil {
			return err
		}
		if !strings.Contains(ui.ETag, "-") {
			return failf("expected a multipart upload, got ETag %q", ui.ETag)
		}
		obj, err := c.GetObject(ctx, b, "hl/obj", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		defer obj.Close()
		got, err := io.ReadAll(obj)
		if err != nil {
			return err
		}
		return eqBytes("download", got, data)
	}
	m["presigned_get"] = func(ctx context.Context) error {
		data := []byte("presigned get body")
		if _, err := put(ctx, b, "ps/get", data, minio.PutObjectOptions{}); err != nil {
			return err
		}
		u, err := c.PresignedGetObject(ctx, b, "ps/get", 15*time.Minute, nil)
		if err != nil {
			return err
		}
		st, got, err := httpDo(ctx, http.MethodGet, u.String(), nil, nil)
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
		u, err := c.PresignedPutObject(ctx, b, "ps/put", 15*time.Minute)
		if err != nil {
			return err
		}
		st, body, err := httpDo(ctx, http.MethodPut, u.String(), data, nil)
		if err != nil {
			return err
		}
		if st != 200 {
			return failf("HTTP %d: %s", st, firstLine(body))
		}
		got, _, _, err := get(ctx, b, "ps/put", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	}
	ckTypes := map[string]minio.ChecksumType{"CRC32": minio.ChecksumCRC32, "CRC32C": minio.ChecksumCRC32C,
		"SHA1": minio.ChecksumSHA1, "SHA256": minio.ChecksumSHA256, "CRC64NVME": minio.ChecksumCRC64NVME}
	for id, alg := range checksumScenarios {
		m[id] = func(ctx context.Context) error {
			data := []byte("checksum payload for " + alg)
			k := "ck/" + alg
			want := checksumB64(alg, data)
			if _, err := ck.PutObject(ctx, b, k, bytes.NewReader(data), int64(len(data)), minio.PutObjectOptions{Checksum: ckTypes[alg], DisableMultipart: true}); err != nil {
				return err
			}
			got, info, _, err := get(ctx, b, k, minio.GetObjectOptions{Checksum: true})
			if err != nil {
				return err
			}
			gotSum := map[string]string{"CRC32": info.ChecksumCRC32, "CRC32C": info.ChecksumCRC32C, "SHA1": info.ChecksumSHA1,
				"SHA256": info.ChecksumSHA256, "CRC64NVME": info.ChecksumCRC64NVME}[alg]
			return firstErr(eqBytes("GET", got, data), eqStr("GET checksum "+alg, gotSum, want))
		}
	}
	m["content_md5_bad"] = func(ctx context.Context) error {
		data := []byte("payload")
		_, err := core.PutObject(ctx, b, "badmd5", bytes.NewReader(data), int64(len(data)), md5B64([]byte("something else")), "", minio.PutObjectOptions{})
		return expectErr("PUT", err, []int{400}, "BadDigest", "InvalidDigest")
	}
	m["bad_credentials"] = func(ctx context.Context) error {
		bad := newMinioClient(env.Config, "wrong-secret-key", false)
		_, err := bad.ListBuckets(ctx)
		return expectErr("ListBuckets", err, []int{403}, "SignatureDoesNotMatch", "InvalidAccessKeyId")
	}
	m["delete_bucket_not_empty"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "nonempty", []byte("x"), minio.PutObjectOptions{}); err != nil {
			return err
		}
		return expectErr("DeleteBucket", c.RemoveBucket(ctx, b), []int{409}, "BucketNotEmpty")
	}
	m["versioning"] = func(ctx context.Context) error {
		vb := b + "-v"
		if err := c.MakeBucket(ctx, vb, minio.MakeBucketOptions{}); err != nil {
			return err
		}
		if err := c.EnableVersioning(ctx, vb); err != nil {
			return err
		}
		u1, err := put(ctx, vb, "doc", []byte("version one"), minio.PutObjectOptions{})
		if err != nil {
			return err
		}
		if _, err := put(ctx, vb, "doc", []byte("version two"), minio.PutObjectOptions{}); err != nil {
			return err
		}
		vers, err := listKeys(ctx, vb, minio.ListObjectsOptions{Prefix: "doc", WithVersions: true, Recursive: true})
		if err != nil {
			return err
		}
		if len(vers) != 2 {
			return failf("ListObjectVersions returned %d versions, want 2", len(vers))
		}
		if u1.VersionID == "" || u1.VersionID == "null" {
			return failf("first PUT returned no VersionId")
		}
		if vers[0].VersionID == vers[1].VersionID || (vers[0].VersionID != u1.VersionID && vers[1].VersionID != u1.VersionID) {
			return failf("version ids not distinct or first version missing")
		}
		got, _, _, err := get(ctx, vb, "doc", minio.GetObjectOptions{VersionID: u1.VersionID})
		if err != nil {
			return err
		}
		if err := eqBytes("GET old version", got, []byte("version one")); err != nil {
			return err
		}
		latest, _, _, err := get(ctx, vb, "doc", minio.GetObjectOptions{})
		if err != nil {
			return err
		}
		return eqBytes("GET latest", latest, []byte("version two"))
	}
	m["object_tagging"] = func(ctx context.Context) error {
		if _, err := put(ctx, b, "tagged", []byte("t"), minio.PutObjectOptions{}); err != nil {
			return err
		}
		t, err := tags.NewTags(map[string]string{"env": "test", "team": "s3 compat"}, true)
		if err != nil {
			return err
		}
		if err := c.PutObjectTagging(ctx, b, "tagged", t, minio.PutObjectTaggingOptions{}); err != nil {
			return err
		}
		gt, err := c.GetObjectTagging(ctx, b, "tagged", minio.GetObjectTaggingOptions{})
		if err != nil {
			return err
		}
		return eqStr("tags", fmt.Sprint(gt.ToMap()), "map[env:test team:s3 compat]")
	}
	m["delete_bucket"] = func(ctx context.Context) error {
		objs := c.ListObjects(ctx, b, minio.ListObjectsOptions{Recursive: true, WithVersions: true})
		for e := range c.RemoveObjects(ctx, b, objs, minio.RemoveObjectsOptions{}) {
			return fmt.Errorf("emptying bucket %s: %w", e.ObjectName, e.Err)
		}
		for u := range c.ListIncompleteUploads(ctx, b, "", true) {
			if u.Err == nil {
				_ = c.RemoveIncompleteUpload(ctx, b, u.Key)
			}
		}
		if err := c.RemoveBucket(ctx, b); err != nil {
			return err
		}
		ok, err := c.BucketExists(ctx, b)
		if err != nil {
			return err
		}
		if ok {
			return failf("bucket still exists after delete")
		}
		return nil
	}
	return m
}
