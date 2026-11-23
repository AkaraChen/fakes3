package io.github.akarachen.fakes3;

/**
 * Thrown when fakes3 can't be found, downloaded or started, or when a control call fails. For a
 * control call that the server rejected, {@link #code()} and {@link #status()} carry the server's
 * error code (such as {@code NotFound}) and HTTP status.
 */
public class FakeS3Exception extends RuntimeException {
  private static final long serialVersionUID = 1L;

  private final String code;
  private final int status;

  public FakeS3Exception(String message) {
    this(message, null, 0, null);
  }

  public FakeS3Exception(String message, Throwable cause) {
    this(message, null, 0, cause);
  }

  FakeS3Exception(String message, String code, int status, Throwable cause) {
    super(message, cause);
    this.code = code;
    this.status = status;
  }

  /** The control plane's error code, such as {@code NotFound} or {@code Conflict}; null otherwise. */
  public String code() {
    return code;
  }

  /** The control plane's HTTP status; 0 when the error didn't come from the server. */
  public int status() {
    return status;
  }
}
