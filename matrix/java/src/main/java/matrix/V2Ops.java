package matrix;

import software.amazon.awssdk.core.ResponseBytes;
import software.amazon.awssdk.core.async.AsyncRequestBody;
import software.amazon.awssdk.core.async.AsyncResponseTransformer;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.core.sync.ResponseTransformer;
import software.amazon.awssdk.services.s3.S3AsyncClient;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.*;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;

/** Uniform view over S3Client (sync) and S3AsyncClient (CRT) so one suite drives both. */
interface V2Ops extends AutoCloseable {
    CreateBucketResponse createBucket(CreateBucketRequest r);
    HeadBucketResponse headBucket(HeadBucketRequest r);
    ListBucketsResponse listBuckets(ListBucketsRequest r);
    DeleteBucketResponse deleteBucket(DeleteBucketRequest r);
    PutObjectResponse putObject(PutObjectRequest r, byte[] data);
    ResponseBytes<GetObjectResponse> getObject(GetObjectRequest r);
    HeadObjectResponse headObject(HeadObjectRequest r);
    DeleteObjectResponse deleteObject(DeleteObjectRequest r);
    DeleteObjectsResponse deleteObjects(DeleteObjectsRequest r);
    ListObjectsV2Response listObjectsV2(ListObjectsV2Request r);
    ListObjectsResponse listObjects(ListObjectsRequest r);
    CopyObjectResponse copyObject(CopyObjectRequest r);
    CreateMultipartUploadResponse createMultipartUpload(CreateMultipartUploadRequest r);
    UploadPartResponse uploadPart(UploadPartRequest r, byte[] data);
    CompleteMultipartUploadResponse completeMultipartUpload(CompleteMultipartUploadRequest r);
    AbortMultipartUploadResponse abortMultipartUpload(AbortMultipartUploadRequest r);
    ListPartsResponse listParts(ListPartsRequest r);
    PutBucketVersioningResponse putBucketVersioning(PutBucketVersioningRequest r);
    ListObjectVersionsResponse listObjectVersions(ListObjectVersionsRequest r);
    PutObjectTaggingResponse putObjectTagging(PutObjectTaggingRequest r);
    GetObjectTaggingResponse getObjectTagging(GetObjectTaggingRequest r);

    @Override
    default void close() {}

    static V2Ops sync(S3Client c) {
        return new V2Ops() {
            public CreateBucketResponse createBucket(CreateBucketRequest r) { return c.createBucket(r); }
            public HeadBucketResponse headBucket(HeadBucketRequest r) { return c.headBucket(r); }
            public ListBucketsResponse listBuckets(ListBucketsRequest r) { return c.listBuckets(r); }
            public DeleteBucketResponse deleteBucket(DeleteBucketRequest r) { return c.deleteBucket(r); }
            public PutObjectResponse putObject(PutObjectRequest r, byte[] d) { return c.putObject(r, RequestBody.fromBytes(d)); }
            public ResponseBytes<GetObjectResponse> getObject(GetObjectRequest r) { return c.getObject(r, ResponseTransformer.toBytes()); }
            public HeadObjectResponse headObject(HeadObjectRequest r) { return c.headObject(r); }
            public DeleteObjectResponse deleteObject(DeleteObjectRequest r) { return c.deleteObject(r); }
            public DeleteObjectsResponse deleteObjects(DeleteObjectsRequest r) { return c.deleteObjects(r); }
            public ListObjectsV2Response listObjectsV2(ListObjectsV2Request r) { return c.listObjectsV2(r); }
            public ListObjectsResponse listObjects(ListObjectsRequest r) { return c.listObjects(r); }
            public CopyObjectResponse copyObject(CopyObjectRequest r) { return c.copyObject(r); }
            public CreateMultipartUploadResponse createMultipartUpload(CreateMultipartUploadRequest r) { return c.createMultipartUpload(r); }
            public UploadPartResponse uploadPart(UploadPartRequest r, byte[] d) { return c.uploadPart(r, RequestBody.fromBytes(d)); }
            public CompleteMultipartUploadResponse completeMultipartUpload(CompleteMultipartUploadRequest r) { return c.completeMultipartUpload(r); }
            public AbortMultipartUploadResponse abortMultipartUpload(AbortMultipartUploadRequest r) { return c.abortMultipartUpload(r); }
            public ListPartsResponse listParts(ListPartsRequest r) { return c.listParts(r); }
            public PutBucketVersioningResponse putBucketVersioning(PutBucketVersioningRequest r) { return c.putBucketVersioning(r); }
            public ListObjectVersionsResponse listObjectVersions(ListObjectVersionsRequest r) { return c.listObjectVersions(r); }
            public PutObjectTaggingResponse putObjectTagging(PutObjectTaggingRequest r) { return c.putObjectTagging(r); }
            public GetObjectTaggingResponse getObjectTagging(GetObjectTaggingRequest r) { return c.getObjectTagging(r); }
            public void close() { c.close(); }
        };
    }

    static <T> T join(CompletableFuture<T> f) {
        try {
            return f.join();
        } catch (CompletionException e) {
            Throwable c = U.unwrap(e);
            if (c instanceof RuntimeException re) throw re;
            if (c instanceof Error er) throw er;
            throw e;
        }
    }

    static V2Ops async(S3AsyncClient c) {
        return new V2Ops() {
            public CreateBucketResponse createBucket(CreateBucketRequest r) { return join(c.createBucket(r)); }
            public HeadBucketResponse headBucket(HeadBucketRequest r) { return join(c.headBucket(r)); }
            public ListBucketsResponse listBuckets(ListBucketsRequest r) { return join(c.listBuckets(r)); }
            public DeleteBucketResponse deleteBucket(DeleteBucketRequest r) { return join(c.deleteBucket(r)); }
            public PutObjectResponse putObject(PutObjectRequest r, byte[] d) { return join(c.putObject(r, AsyncRequestBody.fromBytes(d))); }
            public ResponseBytes<GetObjectResponse> getObject(GetObjectRequest r) { return join(c.getObject(r, AsyncResponseTransformer.toBytes())); }
            public HeadObjectResponse headObject(HeadObjectRequest r) { return join(c.headObject(r)); }
            public DeleteObjectResponse deleteObject(DeleteObjectRequest r) { return join(c.deleteObject(r)); }
            public DeleteObjectsResponse deleteObjects(DeleteObjectsRequest r) { return join(c.deleteObjects(r)); }
            public ListObjectsV2Response listObjectsV2(ListObjectsV2Request r) { return join(c.listObjectsV2(r)); }
            public ListObjectsResponse listObjects(ListObjectsRequest r) { return join(c.listObjects(r)); }
            public CopyObjectResponse copyObject(CopyObjectRequest r) { return join(c.copyObject(r)); }
            public CreateMultipartUploadResponse createMultipartUpload(CreateMultipartUploadRequest r) { return join(c.createMultipartUpload(r)); }
            public UploadPartResponse uploadPart(UploadPartRequest r, byte[] d) { return join(c.uploadPart(r, AsyncRequestBody.fromBytes(d))); }
            public CompleteMultipartUploadResponse completeMultipartUpload(CompleteMultipartUploadRequest r) { return join(c.completeMultipartUpload(r)); }
            public AbortMultipartUploadResponse abortMultipartUpload(AbortMultipartUploadRequest r) { return join(c.abortMultipartUpload(r)); }
            public ListPartsResponse listParts(ListPartsRequest r) { return join(c.listParts(r)); }
            public PutBucketVersioningResponse putBucketVersioning(PutBucketVersioningRequest r) { return join(c.putBucketVersioning(r)); }
            public ListObjectVersionsResponse listObjectVersions(ListObjectVersionsRequest r) { return join(c.listObjectVersions(r)); }
            public PutObjectTaggingResponse putObjectTagging(PutObjectTaggingRequest r) { return join(c.putObjectTagging(r)); }
            public GetObjectTaggingResponse getObjectTagging(GetObjectTaggingRequest r) { return join(c.getObjectTagging(r)); }
            public void close() { c.close(); }
        };
    }
}
