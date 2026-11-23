package io.github.akarachen.fakes3;

import java.time.Instant;
import java.util.Map;

/** One S3 request in the server's request log. */
public final class RecordedRequest {
  private long seq;
  private Instant time;
  private String method;
  private String path;
  private String bucket;
  private String key;
  private String operation;
  private int status;
  private long latencyUs;
  private long bytesIn;
  private long bytesOut;
  private String accessKey;
  private String requestId;

  private RecordedRequest() {}

  static RecordedRequest from(Map<String, Object> m) {
    RecordedRequest r = new RecordedRequest();
    r.seq = FakeS3.number(m.get("seq"));
    r.time = FakeS3.instant(m.get("time"));
    r.method = (String) m.get("method");
    r.path = (String) m.get("path");
    r.bucket = (String) m.get("bucket");
    r.key = (String) m.get("key");
    r.operation = (String) m.get("operation");
    r.status = (int) FakeS3.number(m.get("status"));
    r.latencyUs = FakeS3.number(m.get("latency_us"));
    r.bytesIn = FakeS3.number(m.get("bytes_in"));
    r.bytesOut = FakeS3.number(m.get("bytes_out"));
    r.accessKey = (String) m.get("access_key");
    r.requestId = (String) m.get("request_id");
    return r;
  }

  /** Arrival order, from 1; not reset by clearing the log. */
  public long seq() {
    return seq;
  }

  /** Server time the request arrived. */
  public Instant time() {
    return time;
  }

  public String method() {
    return method;
  }

  /** Path and query string, as sent. */
  public String path() {
    return path;
  }

  /** Decoded bucket name, or null. */
  public String bucket() {
    return bucket;
  }

  /** Decoded object key, or null. */
  public String key() {
    return key;
  }

  /** S3 operation name such as {@code PutObject}; empty when not recognized. */
  public String operation() {
    return operation;
  }

  public int status() {
    return status;
  }

  /** Microseconds until the response headers were ready. */
  public long latencyUs() {
    return latencyUs;
  }

  public long bytesIn() {
    return bytesIn;
  }

  public long bytesOut() {
    return bytesOut;
  }

  /** The access key the request was signed with, or null. */
  public String accessKey() {
    return accessKey;
  }

  /** The {@code x-amz-request-id} the server answered with. */
  public String requestId() {
    return requestId;
  }

  @Override
  public String toString() {
    return "#" + seq + " " + method + " " + path + " " + operation + " -> " + status;
  }
}
