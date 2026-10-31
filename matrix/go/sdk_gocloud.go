package main

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"slices"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/s3/types"
	"gocloud.dev/blob"
	"gocloud.dev/blob/s3blob"
	"gocloud.dev/gcerrors"
)

// ---------- gocloud.dev/blob (s3blob driver) ----------
//
// The portable blob API has no bucket management, multipart primitives,
// batch delete, checksums, versioning or tagging. Where gocloud documents an
// As()-based escape hatch to the underlying request (BeforeRead/BeforeList/
// BeforeCopy) we use it, since that is the supported way to reach those knobs.

type GocloudSDK struct{}

func (GocloudSDK) Name() string   { return "gocloud-blob" }
func (GocloudSDK) Module() string { return "gocloud.dev" }

func gcOpen(ctx context.Context, cfg Config, secret, bucket string, legacy bool) (*blob.Bucket, error) {
	return s3blob.OpenBucket(ctx, newV2Client(cfg, secret), bucket, &s3blob.Options{UseLegacyList: legacy})
}

// gcIs checks the gocloud portable error code, falling back to HTTP status / S3 code.
func gcExpect(what string, err error, code gcerrors.ErrorCode, statuses []int, codes ...string) error {
	if err != nil && gcerrors.Code(err) == code {
		return nil
	}
	return expectErr(what, err, statuses, codes...)
}

func (GocloudSDK) Scenarios(env *Env) map[string]func(ctx context.Context) error {
	m := map[string]func(ctx context.Context) error{}
	open := func(ctx context.Context) (*blob.Bucket, error) {
		return gcOpen(ctx, env.Config, env.SecretKey, env.Bucket, false)
	}
	// with wraps a scenario that needs the main bucket handle.
	with := func(fn func(ctx context.Context, bk *blob.Bucket) error) func(ctx context.Context) error {
		return func(ctx context.Context) error {
			bk, err := open(ctx)
			if err != nil {
				return err
			}
			defer bk.Close()
			return fn(ctx, bk)
		}
	}
	skip := func(reason string) func(ctx context.Context) error {
		return func(context.Context) error { return Skip(reason) }
	}

	noBucketAdmin := "gocloud blob has no bucket management API (create/list/delete buckets)"
	m["create_bucket"] = skip(noBucketAdmin)
	m["list_buckets"] = skip(noBucketAdmin)
	m["delete_bucket"] = skip(noBucketAdmin)
	m["delete_bucket_not_empty"] = skip(noBucketAdmin)
	m["head_bucket"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		ok, err := bk.IsAccessible(ctx)
		if err != nil {
			return err
		}
		if !ok {
			return failf("IsAccessible returned false")
		}
		return nil
	})
	m["head_bucket_missing"] = func(ctx context.Context) error {
		bk, err := gcOpen(ctx, env.Config, env.SecretKey, "m-missing-"+randHex(6), false)
		if err != nil {
			return err
		}
		defer bk.Close()
		ok, err := bk.IsAccessible(ctx)
		if err != nil {
			return gcExpect("IsAccessible", err, gcerrors.NotFound, []int{404}, "NoSuchBucket", "NotFound")
		}
		if ok {
			return failf("IsAccessible returned true for a missing bucket")
		}
		return nil
	}
	m["put_get_small"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := []byte("hello world")
		if err := bk.WriteAll(ctx, "small.txt", data, nil); err != nil {
			return err
		}
		got, err := bk.ReadAll(ctx, "small.txt")
		if err != nil {
			return err
		}
		a, err := bk.Attributes(ctx, "small.txt")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("ETag", unquote(a.ETag), md5Hex(data)), eqStr("MD5", hex.EncodeToString(a.MD5), md5Hex(data)))
	})
	m["put_get_empty"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "empty", nil, nil); err != nil {
			return err
		}
		r, err := bk.NewReader(ctx, "empty", nil)
		if err != nil {
			return err
		}
		defer r.Close()
		got, err := io.ReadAll(r)
		if err != nil {
			return err
		}
		var out s3.GetObjectOutput
		cl := int64(-1)
		if r.As(&out) {
			cl = aws.ToInt64(out.ContentLength)
		}
		if len(got) != 0 || r.Size() != 0 || cl != 0 {
			return failf("expected empty body, got %d bytes, size %d, Content-Length %d", len(got), r.Size(), cl)
		}
		return nil
	})
	m["put_get_large"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := randBytes(8 << 20)
		// BufferSize > object size => single PutObject.
		if err := bk.WriteAll(ctx, "large.bin", data, &blob.WriterOptions{BufferSize: 16 << 20}); err != nil {
			return err
		}
		got, err := bk.ReadAll(ctx, "large.bin")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	})
	m["special_keys"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		for _, k := range specialKeys {
			data := []byte("key:" + k)
			if err := bk.WriteAll(ctx, k, data, nil); err != nil {
				return fmt.Errorf("PUT %q: %w", k, err)
			}
			got, err := bk.ReadAll(ctx, k)
			if err != nil {
				return fmt.Errorf("GET %q: %w", k, err)
			}
			if err := eqBytes("GET "+k, got, data); err != nil {
				return err
			}
		}
		var keys []string
		it := bk.List(nil)
		for {
			o, err := it.Next(ctx)
			if err == io.EOF {
				break
			}
			if err != nil {
				return err
			}
			keys = append(keys, o.Key)
		}
		for _, k := range specialKeys {
			if !slices.Contains(keys, k) {
				return failf("key %q missing from list", k)
			}
		}
		return nil
	})
	m["head_object_metadata"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		err := bk.WriteAll(ctx, "meta.txt", []byte("meta"), &blob.WriterOptions{ContentType: mdContentType, CacheControl: mdCacheControl,
			ContentDisposition: mdContentDisposition, ContentEncoding: mdContentEncoding, Metadata: metaIn})
		if err != nil {
			return err
		}
		a, err := bk.Attributes(ctx, "meta.txt")
		if err != nil {
			return err
		}
		return firstErr(
			eqStr("Content-Type", a.ContentType, mdContentType),
			eqStr("Cache-Control", a.CacheControl, mdCacheControl),
			eqStr("Content-Disposition", a.ContentDisposition, mdContentDisposition),
			eqStr("Content-Encoding", a.ContentEncoding, mdContentEncoding),
			checkMeta(a.Metadata))
	})
	readRange := func(ctx context.Context, bk *blob.Bucket, key string, off, n int64, opts *blob.ReaderOptions) ([]byte, string, error) {
		r, err := bk.NewRangeReader(ctx, key, off, n, opts)
		if err != nil {
			return nil, "", err
		}
		defer r.Close()
		got, err := io.ReadAll(r)
		var out s3.GetObjectOutput
		cr := ""
		if r.As(&out) {
			cr = aws.ToString(out.ContentRange)
		}
		return got, cr, err
	}
	m["get_range"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := randBytes(100)
		if err := bk.WriteAll(ctx, "range.bin", data, nil); err != nil {
			return err
		}
		suffix := &blob.ReaderOptions{BeforeRead: func(as func(any) bool) error {
			// gocloud has no suffix-range API; set it on the underlying request.
			var in *s3.GetObjectInput
			if as(&in) {
				in.Range = aws.String("bytes=-5")
			}
			return nil
		}}
		cases := []struct {
			off, n   int64
			opts     *blob.ReaderOptions
			name, cr string
			want     []byte
		}{
			{10, 10, nil, "bytes=10-19", "bytes 10-19/100", data[10:20]},
			{0, -1, suffix, "bytes=-5", "bytes 95-99/100", data[95:]},
			{95, -1, nil, "bytes=95-", "bytes 95-99/100", data[95:]},
		}
		for _, tc := range cases {
			got, cr, err := readRange(ctx, bk, "range.bin", tc.off, tc.n, tc.opts)
			if err != nil {
				return fmt.Errorf("%s: %w", tc.name, err)
			}
			if err := firstErr(eqBytes(tc.name, got, tc.want), eqStr(tc.name+" Content-Range", cr, tc.cr)); err != nil {
				return err
			}
		}
		return nil
	})
	m["get_range_invalid"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "range-inv.bin", randBytes(100), nil); err != nil {
			return err
		}
		_, _, err := readRange(ctx, bk, "range-inv.bin", 200, 101, nil)
		return expectErr("GET bytes=200-300", err, []int{416}, "InvalidRange")
	})
	m["get_missing_key"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		_, err := bk.ReadAll(ctx, "missing-"+randHex(4))
		return gcExpect("GET", err, gcerrors.NotFound, []int{404}, "NoSuchKey")
	})
	condRead := func(set func(in *s3.GetObjectInput)) *blob.ReaderOptions {
		return &blob.ReaderOptions{BeforeRead: func(as func(any) bool) error {
			var in *s3.GetObjectInput
			if !as(&in) {
				return errors.New("BeforeRead: GetObjectInput not available")
			}
			set(in)
			return nil
		}}
	}
	m["get_if_none_match"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "cond.txt", []byte("conditional"), nil); err != nil {
			return err
		}
		a, err := bk.Attributes(ctx, "cond.txt")
		if err != nil {
			return err
		}
		_, _, err = readRange(ctx, bk, "cond.txt", 0, -1, condRead(func(in *s3.GetObjectInput) { in.IfNoneMatch = aws.String(a.ETag) }))
		return expectErr("GET If-None-Match", err, []int{304}, "NotModified")
	})
	m["get_if_match_fail"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "cond2.txt", []byte("conditional"), nil); err != nil {
			return err
		}
		_, _, err := readRange(ctx, bk, "cond2.txt", 0, -1, condRead(func(in *s3.GetObjectInput) { in.IfMatch = aws.String(`"wrong"`) }))
		return gcExpect("GET If-Match", err, gcerrors.FailedPrecondition, []int{412}, "PreconditionFailed")
	})
	m["put_if_none_match"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		k := "ifnm-" + randHex(4)
		doPut := func() error { return bk.WriteAll(ctx, k, []byte("x"), &blob.WriterOptions{IfNotExist: true}) }
		if err := doPut(); err != nil {
			return fmt.Errorf("first PUT (new key): %w", err)
		}
		return gcExpect("second PUT", doPut(), gcerrors.FailedPrecondition, []int{412}, "PreconditionFailed")
	})
	m["delete_object"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "del.txt", []byte("x"), nil); err != nil {
			return err
		}
		if err := bk.Delete(ctx, "del.txt"); err != nil {
			return err
		}
		_, err := bk.Attributes(ctx, "del.txt")
		return gcExpect("HEAD after delete", err, gcerrors.NotFound, []int{404}, "NotFound", "NoSuchKey")
	})
	m["delete_missing_object"] = skip("gocloud Delete HEADs the key first and returns NotFound by contract; it never issues DELETE for a missing key")
	m["delete_objects"] = skip("gocloud blob has no batch delete API")
	m["list_v2_basic"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		keys := []string{"lb/a", "lb/b", "lb/c"}
		datas := [][]byte{randBytes(1), randBytes(22), randBytes(333)}
		for i := len(keys) - 1; i >= 0; i-- {
			if err := bk.WriteAll(ctx, keys[i], datas[i], nil); err != nil {
				return err
			}
		}
		objs, _, err := bk.ListPage(ctx, blob.FirstPageToken, 1000, &blob.ListOptions{Prefix: "lb/"})
		if err != nil {
			return err
		}
		if len(objs) != 3 {
			return failf("got %d objects, want 3", len(objs))
		}
		for i, o := range objs {
			if err := firstErr(eqStr("key", o.Key, keys[i]), eqStr("etag "+keys[i], hex.EncodeToString(o.MD5), md5Hex(datas[i]))); err != nil {
				return err
			}
			if o.Size != int64(len(datas[i])) {
				return failf("size %s: got %d want %d", keys[i], o.Size, len(datas[i]))
			}
		}
		return nil
	})
	m["list_v2_prefix_delimiter"] = func(ctx context.Context) error {
		db := env.Bucket + "-d"
		if err := adminEnsureBucket(ctx, env.Config, db); err != nil {
			return fmt.Errorf("setup (aws-sdk-go-v2 CreateBucket): %w", err)
		}
		bk, err := gcOpen(ctx, env.Config, env.SecretKey, db, false)
		if err != nil {
			return err
		}
		defer bk.Close()
		for _, k := range []string{"a/1", "a/2", "b/1", "c"} {
			if err := bk.WriteAll(ctx, k, []byte(k), nil); err != nil {
				return err
			}
		}
		objs, _, err := bk.ListPage(ctx, blob.FirstPageToken, 1000, &blob.ListOptions{Delimiter: "/"})
		if err != nil {
			return err
		}
		var cps, cs []string
		for _, o := range objs {
			if o.IsDir {
				cps = append(cps, o.Key)
			} else {
				cs = append(cs, o.Key)
			}
		}
		return firstErr(eqStr("CommonPrefixes", fmt.Sprint(cps), "[a/ b/]"), eqStr("Contents", fmt.Sprint(cs), "[c]"))
	}
	m["list_v2_pagination"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		for _, k := range paginationKeys("pg/") {
			if err := bk.WriteAll(ctx, k, []byte("p"), nil); err != nil {
				return err
			}
		}
		var all []string
		pages := 0
		token := blob.FirstPageToken
		for {
			objs, next, err := bk.ListPage(ctx, token, 10, &blob.ListOptions{Prefix: "pg/"})
			if err != nil {
				return err
			}
			pages++
			for _, o := range objs {
				all = append(all, o.Key)
			}
			if next == nil {
				break
			}
			if pages > 10 {
				return failf("too many pages")
			}
			token = next
		}
		if pages != 3 || len(all) != 25 {
			return failf("got %d pages / %d keys, want 3 / 25", pages, len(all))
		}
		return sortedUnique(all)
	})
	m["list_v2_start_after"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		for _, k := range []string{"sa/0", "sa/1", "sa/2", "sa/3", "sa/4"} {
			if err := bk.WriteAll(ctx, k, []byte("s"), nil); err != nil {
				return err
			}
		}
		objs, _, err := bk.ListPage(ctx, blob.FirstPageToken, 1000, &blob.ListOptions{Prefix: "sa/",
			BeforeList: func(as func(any) bool) error {
				// No StartAfter in the portable API; set it on the ListObjectsV2 request.
				var in *s3.ListObjectsV2Input
				if !as(&in) {
					return errors.New("BeforeList: ListObjectsV2Input not available")
				}
				in.StartAfter = aws.String("sa/2")
				return nil
			}})
		if err != nil {
			return err
		}
		var got []string
		for _, o := range objs {
			got = append(got, o.Key)
		}
		return eqStr("keys", fmt.Sprint(got), "[sa/3 sa/4]")
	})
	m["list_v1"] = func(ctx context.Context) error {
		// s3blob's UseLegacyList switches to ListObjects (v1); the page token is the Marker.
		bk, err := gcOpen(ctx, env.Config, env.SecretKey, env.Bucket, true)
		if err != nil {
			return err
		}
		defer bk.Close()
		for _, k := range []string{"l1/0", "l1/1", "l1/2", "l1/3", "l1/4"} {
			if err := bk.WriteAll(ctx, k, []byte("v1"), nil); err != nil {
				return err
			}
		}
		var got []string
		token := []byte("l1/0")
		for i := 0; i < 10 && token != nil; i++ {
			objs, next, err := bk.ListPage(ctx, token, 2, &blob.ListOptions{Prefix: "l1/"})
			if err != nil {
				return err
			}
			for _, o := range objs {
				got = append(got, o.Key)
			}
			token = next
		}
		return eqStr("keys after marker", fmt.Sprint(got), "[l1/1 l1/2 l1/3 l1/4]")
	}
	m["copy_object"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := []byte("copy me")
		if err := bk.WriteAll(ctx, "cp/src", data, &blob.WriterOptions{ContentType: "text/x-copy", Metadata: metaIn}); err != nil {
			return err
		}
		if err := bk.Copy(ctx, "cp/dst", "cp/src", nil); err != nil {
			return err
		}
		got, err := bk.ReadAll(ctx, "cp/dst")
		if err != nil {
			return err
		}
		a, err := bk.Attributes(ctx, "cp/dst")
		if err != nil {
			return err
		}
		return firstErr(eqBytes("GET", got, data), eqStr("Content-Type", a.ContentType, "text/x-copy"), checkMeta(a.Metadata))
	})
	m["copy_object_replace_metadata"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		if err := bk.WriteAll(ctx, "cpr/src", []byte("x"), &blob.WriterOptions{ContentType: "text/x-old", Metadata: metaIn}); err != nil {
			return err
		}
		err := bk.Copy(ctx, "cpr/dst", "cpr/src", &blob.CopyOptions{BeforeCopy: func(as func(any) bool) error {
			// Portable Copy always preserves metadata; REPLACE needs the escape hatch.
			var in *s3.CopyObjectInput
			if !as(&in) {
				return errors.New("BeforeCopy: CopyObjectInput not available")
			}
			in.MetadataDirective = types.MetadataDirectiveReplace
			in.ContentType = aws.String("text/x-new")
			in.Metadata = map[string]string{"color": "Red", "fresh": "yes"}
			return nil
		}})
		if err != nil {
			return err
		}
		a, err := bk.Attributes(ctx, "cpr/dst")
		if err != nil {
			return err
		}
		return firstErr(eqStr("Content-Type", a.ContentType, "text/x-new"), eqStr("metadata", fmt.Sprint(a.Metadata), "map[color:Red fresh:yes]"))
	})
	noMP := "gocloud blob has no multipart upload primitives (only the streaming Writer)"
	m["multipart_upload"] = skip(noMP)
	m["multipart_abort"] = skip(noMP)
	m["multipart_list_parts"] = skip(noMP)
	m["multipart_too_small"] = skip(noMP)
	m["high_level_upload"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := randBytes(20 << 20)
		w, err := bk.NewWriter(ctx, "hl/obj", &blob.WriterOptions{BufferSize: 5 << 20})
		if err != nil {
			return err
		}
		for off := 0; off < len(data); off += 1 << 20 {
			if _, err := w.Write(data[off : off+1<<20]); err != nil {
				w.Close()
				return err
			}
		}
		if err := w.Close(); err != nil {
			return err
		}
		var buf = make([]byte, 0, len(data))
		r, err := bk.NewReader(ctx, "hl/obj", nil)
		if err != nil {
			return err
		}
		defer r.Close()
		buf, err = io.ReadAll(r)
		if err != nil {
			return err
		}
		return eqBytes("download", buf, data)
	})
	m["presigned_get"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := []byte("presigned get body")
		if err := bk.WriteAll(ctx, "ps/get", data, nil); err != nil {
			return err
		}
		u, err := bk.SignedURL(ctx, "ps/get", &blob.SignedURLOptions{Expiry: 15 * time.Minute, Method: http.MethodGet})
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
	})
	m["presigned_put"] = with(func(ctx context.Context, bk *blob.Bucket) error {
		data := []byte("presigned put body")
		u, err := bk.SignedURL(ctx, "ps/put", &blob.SignedURLOptions{Expiry: 15 * time.Minute, Method: http.MethodPut})
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
		got, err := bk.ReadAll(ctx, "ps/put")
		if err != nil {
			return err
		}
		return eqBytes("GET", got, data)
	})
	for id := range checksumScenarios {
		m[id] = skip("gocloud blob has no flexible-checksum API")
	}
	m["content_md5_bad"] = skip("s3blob ignores WriterOptions.ContentMD5 (no way to send Content-MD5)")
	m["bad_credentials"] = func(ctx context.Context) error {
		// No ListBuckets in gocloud; list the bucket with a wrong secret instead.
		bk, err := gcOpen(ctx, env.Config, "wrong-secret-key", env.Bucket, false)
		if err != nil {
			return err
		}
		defer bk.Close()
		_, _, err = bk.ListPage(ctx, blob.FirstPageToken, 1, nil)
		return gcExpect("List", err, gcerrors.PermissionDenied, []int{403}, "SignatureDoesNotMatch", "InvalidAccessKeyId")
	}
	m["versioning"] = skip("gocloud blob has no versioning API")
	m["object_tagging"] = skip("gocloud blob has no object tagging API")
	return m
}
