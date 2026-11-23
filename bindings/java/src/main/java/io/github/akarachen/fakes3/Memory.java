package io.github.akarachen.fakes3;

import java.util.OptionalLong;

/** Object data the server holds, against its limit. */
public final class Memory {
  private final long used;
  private final OptionalLong limit;

  Memory(long used, OptionalLong limit) {
    this.used = used;
    this.limit = limit;
  }

  /** Bytes of object data held, including multipart parts and data kept alive by snapshots. */
  public long used() {
    return used;
  }

  /** The configured limit in bytes; empty when unlimited. */
  public OptionalLong limit() {
    return limit;
  }

  @Override
  public String toString() {
    return "Memory[used=" + used + ", limit=" + (limit.isPresent() ? limit.getAsLong() : "none") + "]";
  }
}
