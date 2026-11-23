package io.github.akarachen.fakes3;

import java.time.Instant;
import java.util.Map;

/** A saved state of the server. */
public final class Snapshot {
  private final String name;
  private final Instant created;
  private final long buckets;
  private final long objects;
  private final long uploads;

  private Snapshot(String name, Instant created, long buckets, long objects, long uploads) {
    this.name = name;
    this.created = created;
    this.buckets = buckets;
    this.objects = objects;
    this.uploads = uploads;
  }

  static Snapshot from(Map<String, Object> m) {
    return new Snapshot(
        (String) m.get("name"),
        FakeS3.instant(m.get("created")),
        FakeS3.number(m.get("buckets")),
        FakeS3.number(m.get("objects")),
        FakeS3.number(m.get("uploads")));
  }

  public String name() {
    return name;
  }

  /** Server time the snapshot was taken. */
  public Instant created() {
    return created;
  }

  public long buckets() {
    return buckets;
  }

  /** Versioned keys, delete markers included. */
  public long objects() {
    return objects;
  }

  /** Multipart uploads in progress. */
  public long uploads() {
    return uploads;
  }

  @Override
  public String toString() {
    return "Snapshot[" + name + ", buckets=" + buckets + ", objects=" + objects + ", uploads=" + uploads + "]";
  }
}
