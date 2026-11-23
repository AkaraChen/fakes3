package io.github.akarachen.fakes3;

import java.nio.file.Paths;

/** Starts servers and never closes them; see CleanupTest. */
public final class LeakyMain {
  private LeakyMain() {}

  public static void main(String[] args) throws Exception {
    for (int i = 0; i < 2; i++) {
      FakeS3 s3 = FakeS3.builder().binary(Paths.get(args[0])).start();
      System.out.println("pid " + s3.pid());
    }
    System.out.println("ready");
    System.out.flush();
    switch (args[1]) {
      case "exit":
        System.exit(0);
        break;
      case "throw":
        throw new IllegalStateException("test failed");
      default:
        Thread.sleep(60_000);
    }
  }
}
