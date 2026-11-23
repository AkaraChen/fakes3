package io.github.akarachen.fakes3;

import java.util.Map;

/** The request log's settings and fill level. */
public final class RequestLogConfig {
  private final boolean enabled;
  private final long capacity;
  private final long stored;
  private final long dropped;

  private RequestLogConfig(boolean enabled, long capacity, long stored, long dropped) {
    this.enabled = enabled;
    this.capacity = capacity;
    this.stored = stored;
    this.dropped = dropped;
  }

  static RequestLogConfig from(Map<String, Object> m) {
    return new RequestLogConfig(
        Boolean.TRUE.equals(m.get("enabled")),
        FakeS3.number(m.get("capacity")),
        FakeS3.number(m.get("stored")),
        FakeS3.number(m.get("dropped")));
  }

  public boolean enabled() {
    return enabled;
  }

  public long capacity() {
    return capacity;
  }

  /** Records currently held. */
  public long stored() {
    return stored;
  }

  /** Records dropped because the log was full. */
  public long dropped() {
    return dropped;
  }

  @Override
  public String toString() {
    return "RequestLogConfig[enabled=" + enabled + ", capacity=" + capacity + ", stored=" + stored
        + ", dropped=" + dropped + "]";
  }
}
