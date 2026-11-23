package io.github.akarachen.fakes3;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * The log of S3 requests the server received, oldest first. Use it to assert on what your code
 * actually sent. Control calls are not recorded.
 */
public final class Requests {
  private final FakeS3 server;

  Requests(FakeS3 server) {
    this.server = server;
  }

  /** Every recorded request. */
  public List<RecordedRequest> list() {
    return list(RequestFilter.create());
  }

  /** The recorded requests that match {@code filter}, oldest first. */
  public List<RecordedRequest> list(RequestFilter filter) {
    Map<String, Object> m = server.call("GET", "requests" + filter.query(null), null);
    List<RecordedRequest> out = new ArrayList<>();
    for (Object o : FakeS3.list(m.get("requests"))) {
      out.add(RecordedRequest.from(FakeS3.map(o)));
    }
    return Collections.unmodifiableList(out);
  }

  /** How many recorded requests there are. */
  public long count() {
    return count(RequestFilter.create());
  }

  /** How many recorded requests match {@code filter}, ignoring its limit. */
  public long count(RequestFilter filter) {
    return FakeS3.number(server.call("GET", "requests" + filter.query(0), null).get("total"));
  }

  /** Empties the log. Sequence numbers keep counting. */
  public void clear() {
    server.call("DELETE", "requests", null);
  }

  /** Whether the log is recording, its capacity, and how many records it holds. */
  public RequestLogConfig config() {
    return RequestLogConfig.from(server.call("GET", "requests/config", null));
  }

  /** Starts or stops recording. Stopping keeps the records already there. */
  public RequestLogConfig setEnabled(boolean enabled) {
    return RequestLogConfig.from(server.call("PUT", "requests/config", "{\"enabled\":" + enabled + "}"));
  }

  /** How many records to keep; shrinking drops the oldest. */
  public RequestLogConfig setCapacity(int capacity) {
    return RequestLogConfig.from(server.call("PUT", "requests/config", "{\"capacity\":" + capacity + "}"));
  }
}
