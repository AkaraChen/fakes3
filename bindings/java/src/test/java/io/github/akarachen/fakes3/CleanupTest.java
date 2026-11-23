package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.platform.engine.discovery.DiscoverySelectors.selectClass;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.DisabledOnOs;
import org.junit.jupiter.api.condition.OS;
import org.junit.platform.testkit.engine.EngineTestKit;

/** Servers don't outlive their test, their class, or the JVM. */
class CleanupTest {
  @Test
  void closeStopsTheProcess() throws Exception {
    FakeS3 s3 = FakeS3.start();
    long pid = s3.pid();
    assertTrue(alive(pid));
    s3.close();
    s3.close();
    assertFalse(s3.isRunning());
    assertGone(pid);
    IllegalStateException e = assertThrows(IllegalStateException.class, s3::reset);
    assertEquals("server is stopped", e.getMessage());
  }

  @Test
  void failedTestStillStopsItsServer() throws Exception {
    EngineTestKit.engine("junit-jupiter")
        .selectors(selectClass(FailingExample.class))
        .execute()
        .testEvents()
        .assertStatistics(s -> s.started(1).failed(1));
    assertGone(FailingExample.pid);
  }

  @Test
  void jvmExitStopsServers() throws Exception {
    assertEquals(0, childJvm("exit").exitValue);
  }

  @Test
  void uncaughtExceptionStopsServers() throws Exception {
    assertEquals(1, childJvm("throw").exitValue);
  }

  /** SIGTERM runs shutdown hooks; Windows has no equivalent for a console process. */
  @Test
  @DisabledOnOs(OS.WINDOWS)
  void sigtermStopsServers() throws Exception {
    childJvm("wait");
  }

  private static final class Child {
    int exitValue;
  }

  /**
   * Starts a JVM that starts two servers, prints their pids and then ends as {@code how} says,
   * without closing them. Checks that the servers are gone afterwards.
   */
  private static Child childJvm(String how) throws Exception {
    Path binary;
    try (FakeS3 probe = FakeS3.start()) {
      binary = probe.binary();
    }
    Path java = Paths.get(System.getProperty("java.home"), "bin", "java");
    Process p =
        new ProcessBuilder(
                java.toString(),
                "-cp",
                System.getProperty("java.class.path"),
                LeakyMain.class.getName(),
                binary.toString(),
                how)
            .redirectErrorStream(true)
            .start();
    List<Long> pids = new ArrayList<>();
    BufferedReader out = new BufferedReader(new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8));
    String line;
    while ((line = out.readLine()) != null) {
      if (line.startsWith("pid ")) {
        pids.add(Long.parseLong(line.substring(4)));
      }
      if (line.equals("ready")) {
        break;
      }
    }
    assertEquals(2, pids.size(), "the child started two servers");
    for (long pid : pids) {
      assertTrue(alive(pid));
    }
    if (how.equals("wait")) {
      p.destroy();
    }
    assertTrue(p.waitFor(30, TimeUnit.SECONDS), "child JVM exited");
    for (long pid : pids) {
      assertGone(pid);
    }
    Child c = new Child();
    c.exitValue = p.exitValue();
    return c;
  }

  private static boolean alive(long pid) {
    Optional<ProcessHandle> h = ProcessHandle.of(pid);
    return h.isPresent() && h.get().isAlive();
  }

  private static void assertGone(long pid) throws InterruptedException {
    assertNotNull(pid);
    for (int i = 0; i < 100 && alive(pid); i++) {
      Thread.sleep(50);
    }
    assertFalse(alive(pid), "fakes3 process " + pid + " is still running");
  }
}
