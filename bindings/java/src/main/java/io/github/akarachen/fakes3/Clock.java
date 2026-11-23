package io.github.akarachen.fakes3;

import java.time.Duration;
import java.time.Instant;
import java.util.Map;

/**
 * The server clock. Every timestamp the server produces reads it, and so does everything that
 * depends on time: presigned URL expiry, lifecycle rules, Object Lock retention, archive restores.
 *
 * <p>Presigned URLs can be made to expire by moving the clock forward. Signatures are still checked
 * against the host clock, so sign requests with the real time and only move the server clock ahead.
 */
public final class Clock {
  private final FakeS3 server;

  Clock(FakeS3 server) {
    this.server = server;
  }

  /** The current server time. */
  public Instant now() {
    return state().now;
  }

  /** Whether the clock is frozen. */
  public boolean isFrozen() {
    return state().frozen;
  }

  /** Whether the clock differs from the host clock (frozen or moved). */
  public boolean isShifted() {
    return state().shifted;
  }

  /** Stops time at the current server time. Returns the server time. */
  public Instant freeze() {
    return post("freeze", "{}");
  }

  /** Stops time at {@code at}. Returns the server time. */
  public Instant freeze(Instant at) {
    return post("freeze", "{\"at\":" + Json.quote(at.toString()) + "}");
  }

  /** Lets time run again from the current server time. */
  public Instant unfreeze() {
    return post("unfreeze", "{}");
  }

  /** Jumps to {@code at}, keeping the frozen or running state. */
  public Instant set(Instant at) {
    return post("set", "{\"at\":" + Json.quote(at.toString()) + "}");
  }

  /** Moves time forward ({@code by} may be negative). Returns the new server time. */
  public Instant advance(Duration by) {
    return advance(by.toNanos() + "ns");
  }

  /**
   * Moves time forward by a duration such as {@code "30d"}, {@code "1h30m"}, {@code "500ms"} or
   * {@code "-2w"} (units {@code ns us ms s m h d w}).
   */
  public Instant advance(String by) {
    return post("advance", "{\"by\":" + Json.quote(by) + "}");
  }

  /** Back to the host clock. */
  public Instant reset() {
    return post("reset", "{}");
  }

  private Instant post(String op, String body) {
    return State.from(server.call("POST", "clock/" + op, body)).now;
  }

  private State state() {
    return State.from(server.call("GET", "clock", null));
  }

  private static final class State {
    Instant now;
    boolean frozen;
    boolean shifted;

    static State from(Map<String, Object> m) {
      State s = new State();
      s.now = FakeS3.instant(m.get("now"));
      s.frozen = Boolean.TRUE.equals(m.get("frozen"));
      s.shifted = Boolean.TRUE.equals(m.get("shifted"));
      return s;
    }
  }
}
