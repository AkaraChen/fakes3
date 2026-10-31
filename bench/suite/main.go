// suite replays the request pattern of a typical integration-test suite: many short tests, each
// creating a bucket, writing and reading a handful of small objects, listing, copying, then
// tearing everything down. Requests are sequential, so the result is dominated by per-request
// latency, which is what makes a test suite feel fast or slow.
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"sort"
	"time"

	"github.com/aws/aws-sdk-go-v2/aws"
	"github.com/aws/aws-sdk-go-v2/credentials"
	"github.com/aws/aws-sdk-go-v2/service/s3"
	"github.com/aws/aws-sdk-go-v2/service/s3/types"
)

func must(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func main() {
	endpoint := flag.String("endpoint", "http://127.0.0.1:9100", "")
	ak := flag.String("access-key", "fakes3", "")
	sk := flag.String("secret-key", "fakes3secret", "")
	tests := flag.Int("tests", 100, "number of simulated tests")
	objects := flag.Int("objects", 20, "objects per test")
	load := flag.Bool("load", false, "instead of the suite, load a fixed dataset and exit")
	small := flag.Int("small", 10000, "load: number of 4 KiB objects")
	large := flag.Int("large", 8, "load: number of 64 MiB objects")
	flag.Parse()

	ctx := context.Background()
	c := s3.New(s3.Options{
		BaseEndpoint: endpoint,
		Region:       "us-east-1",
		Credentials:  credentials.NewStaticCredentialsProvider(*ak, *sk, ""),
		UsePathStyle: true,
	})
	payload := bytes.Repeat([]byte("x"), 1024)
	if *load {
		loadDataset(ctx, c, *small, *large)
		return
	}

	var durs []float64
	start := time.Now()
	requests := 0
	for t := 0; t < *tests; t++ {
		t0 := time.Now()
		b := aws.String(fmt.Sprintf("suite-%d-%d", os.Getpid(), t))
		_, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: b})
		must(err)
		var ids []types.ObjectIdentifier
		for i := 0; i < *objects; i++ {
			k := aws.String(fmt.Sprintf("dir/obj-%03d.json", i))
			_, err = c.PutObject(ctx, &s3.PutObjectInput{Bucket: b, Key: k, Body: bytes.NewReader(payload), ContentType: aws.String("application/json")})
			must(err)
			ids = append(ids, types.ObjectIdentifier{Key: k})
		}
		out, err := c.ListObjectsV2(ctx, &s3.ListObjectsV2Input{Bucket: b, Prefix: aws.String("dir/")})
		must(err)
		if len(out.Contents) != *objects {
			must(fmt.Errorf("list returned %d objects, want %d", len(out.Contents), *objects))
		}
		for _, id := range ids {
			g, err := c.GetObject(ctx, &s3.GetObjectInput{Bucket: b, Key: id.Key})
			must(err)
			got, err := io.ReadAll(g.Body)
			must(err)
			g.Body.Close()
			if !bytes.Equal(got, payload) {
				must(fmt.Errorf("content mismatch"))
			}
			_, err = c.HeadObject(ctx, &s3.HeadObjectInput{Bucket: b, Key: id.Key})
			must(err)
		}
		_, err = c.CopyObject(ctx, &s3.CopyObjectInput{Bucket: b, Key: aws.String("copy"), CopySource: aws.String(*b + "/" + *ids[0].Key)})
		must(err)
		ids = append(ids, types.ObjectIdentifier{Key: aws.String("copy")})
		_, err = c.DeleteObjects(ctx, &s3.DeleteObjectsInput{Bucket: b, Delete: &types.Delete{Objects: ids}})
		must(err)
		_, err = c.DeleteBucket(ctx, &s3.DeleteBucketInput{Bucket: b})
		must(err)
		requests += 3*(*objects) + 5
		durs = append(durs, float64(time.Since(t0).Microseconds())/1000)
	}
	total := time.Since(start)
	sort.Float64s(durs)
	json.NewEncoder(os.Stdout).Encode(map[string]any{
		"tests":          *tests,
		"requests":       requests,
		"total_ms":       float64(total.Microseconds()) / 1000,
		"per_test_p50":   durs[len(durs)/2],
		"per_test_p99":   durs[len(durs)*99/100],
		"per_request_us": float64(total.Microseconds()) / float64(requests),
	})
}

// loadDataset writes a fixed amount of data so memory footprints are comparable across servers
// regardless of how fast each one is.
func loadDataset(ctx context.Context, c *s3.Client, small, large int) {
	b := aws.String("footprint")
	_, err := c.CreateBucket(ctx, &s3.CreateBucketInput{Bucket: b})
	must(err)
	start := time.Now()
	sem := make(chan struct{}, 16)
	errs := make(chan error, small+large)
	put := func(key string, size int) {
		sem <- struct{}{}
		go func() {
			defer func() { <-sem }()
			buf := make([]byte, size)
			for i := range buf {
				buf[i] = byte(i*31 + len(key))
			}
			_, err := c.PutObject(ctx, &s3.PutObjectInput{Bucket: b, Key: aws.String(key), Body: bytes.NewReader(buf)})
			errs <- err
		}()
	}
	for i := 0; i < small; i++ {
		put(fmt.Sprintf("small/%06d", i), 4096)
	}
	for i := 0; i < large; i++ {
		put(fmt.Sprintf("large/%03d", i), 64<<20)
	}
	for i := 0; i < small+large; i++ {
		must(<-errs)
	}
	json.NewEncoder(os.Stdout).Encode(map[string]any{
		"objects":       small + large,
		"logical_bytes": small*4096 + large*(64<<20),
		"load_ms":       float64(time.Since(start).Microseconds()) / 1000,
	})
}
