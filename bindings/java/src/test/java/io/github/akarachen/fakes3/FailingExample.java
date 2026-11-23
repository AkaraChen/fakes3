package io.github.akarachen.fakes3;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

/** A failing test, run by CleanupTest through EngineTestKit; the build doesn't run it directly. */
@ExtendWith(FakeS3Extension.class)
class FailingExample {
  static long pid;

  @Test
  void fails(FakeS3 s3) {
    pid = s3.pid();
    throw new AssertionError("boom");
  }
}
