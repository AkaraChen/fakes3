package io.github.akarachen.fakes3;

import java.time.Instant;
import java.util.StringJoiner;

/**
 * Which requests {@link Requests#list(RequestFilter)} returns. Immutable; every method returns a
 * new filter. Conditions combine with AND.
 *
 * <pre>{@code
 * s3.requests().list(RequestFilter.create().operation("PutObject").bucket("demo"));
 * }</pre>
 */
public final class RequestFilter {
  private final String bucket;
  private final String keyPrefix;
  private final String operation;
  private final String method;
  private final String status;
  private final Instant since;
  private final Long after;
  private final Integer limit;

  private RequestFilter(
      String bucket,
      String keyPrefix,
      String operation,
      String method,
      String status,
      Instant since,
      Long after,
      Integer limit) {
    this.bucket = bucket;
    this.keyPrefix = keyPrefix;
    this.operation = operation;
    this.method = method;
    this.status = status;
    this.since = since;
    this.after = after;
    this.limit = limit;
  }

  /** A filter that matches every request. */
  public static RequestFilter create() {
    return new RequestFilter(null, null, null, null, null, null, null, null);
  }

  /** Exact bucket name. */
  public RequestFilter bucket(String bucket) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, limit);
  }

  /** Keys starting with {@code prefix}. */
  public RequestFilter keyPrefix(String prefix) {
    return new RequestFilter(bucket, prefix, operation, method, status, since, after, limit);
  }

  /** S3 operation name such as {@code PutObject}, case-insensitive. */
  public RequestFilter operation(String operation) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, limit);
  }

  /** HTTP method, case-insensitive. */
  public RequestFilter method(String method) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, limit);
  }

  /** An exact HTTP status. */
  public RequestFilter status(int status) {
    return status(Integer.toString(status));
  }

  /** {@code "404"}, a class such as {@code "4xx"}, or {@code "error"} (400 and above). */
  public RequestFilter status(String status) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, limit);
  }

  /** Requests at or after this server time. */
  public RequestFilter since(Instant since) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, limit);
  }

  /** Requests with a sequence number greater than {@code seq}: "everything since I last looked". */
  public RequestFilter after(long seq) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, seq, limit);
  }

  /** Keep only the {@code n} most recent matches. */
  public RequestFilter limit(int n) {
    return new RequestFilter(bucket, keyPrefix, operation, method, status, since, after, n);
  }

  String query(Integer overrideLimit) {
    StringJoiner q = new StringJoiner("&", "?", "");
    q.setEmptyValue("");
    add(q, "bucket", bucket);
    add(q, "key_prefix", keyPrefix);
    add(q, "op", operation);
    add(q, "method", method);
    add(q, "status", status);
    add(q, "since", since == null ? null : since.toString());
    add(q, "after", after == null ? null : after.toString());
    Integer l = overrideLimit != null ? overrideLimit : limit;
    add(q, "limit", l == null ? null : l.toString());
    return q.toString();
  }

  private static void add(StringJoiner q, String k, String v) {
    if (v != null) {
      q.add(k + "=" + FakeS3.encode(v));
    }
  }

  @Override
  public String toString() {
    return "RequestFilter" + query(null);
  }
}
