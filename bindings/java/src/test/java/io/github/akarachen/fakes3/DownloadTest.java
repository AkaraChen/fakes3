package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.net.httpserver.HttpServer;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Stream;
import java.util.zip.GZIPOutputStream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** The download path, against a local stand-in for GitHub Releases. */
class DownloadTest {
  private static final byte[] BINARY = "#!/bin/sh\necho not really fakes3\n".getBytes(StandardCharsets.UTF_8);
  private static final Binaries.Platform LINUX = new Binaries.Platform("x86_64-unknown-linux-musl");
  private static final Binaries.Platform WINDOWS = new Binaries.Platform("x86_64-pc-windows-msvc");

  @TempDir Path cache;
  private HttpServer server;
  private URI base;
  private final Map<String, byte[]> files = new ConcurrentHashMap<>();

  @BeforeEach
  void serve() throws IOException {
    server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    server.createContext(
        "/",
        ex -> {
          byte[] body = files.get(ex.getRequestURI().getPath());
          if (body == null) {
            ex.sendResponseHeaders(404, -1);
          } else {
            ex.sendResponseHeaders(200, body.length);
            try (OutputStream out = ex.getResponseBody()) {
              out.write(body);
            }
          }
          ex.close();
        });
    server.start();
    base = URI.create("http://127.0.0.1:" + server.getAddress().getPort() + "/releases");
  }

  @AfterEach
  void stop() {
    server.stop(0);
  }

  private void publish(Binaries.Platform p, byte[] archive, String sha256) {
    String path = "/releases/v9.9.9/" + p.archive;
    files.put(path, archive);
    files.put(path + ".sha256", (sha256 + "  " + p.archive + "\n").getBytes(StandardCharsets.UTF_8));
  }

  @Test
  void installsAVerifiedTarGz() throws Exception {
    byte[] archive = tarGz("fakes3-x86_64-unknown-linux-musl/README.md", "readme".getBytes(StandardCharsets.UTF_8),
        "fakes3-x86_64-unknown-linux-musl/fakes3", BINARY);
    publish(LINUX, archive, sha256(archive));

    Path exe = Binaries.download("9.9.9", LINUX, cache, base);

    assertEquals(cache.resolve("9.9.9").resolve(LINUX.triple).resolve("fakes3"), exe);
    assertArrayEquals(BINARY, Files.readAllBytes(exe));
    // Cached: a second call doesn't go to the network.
    files.clear();
    assertEquals(exe, Binaries.download("9.9.9", LINUX, cache, base));
  }

  @Test
  void installsAVerifiedZip() throws Exception {
    ByteArrayOutputStream bytes = new ByteArrayOutputStream();
    try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
      zip.putNextEntry(new ZipEntry("fakes3-x86_64-pc-windows-msvc/"));
      zip.putNextEntry(new ZipEntry("fakes3-x86_64-pc-windows-msvc/fakes3.exe"));
      zip.write(BINARY);
    }
    byte[] archive = bytes.toByteArray();
    publish(WINDOWS, archive, sha256(archive).toUpperCase());

    Path exe = Binaries.download("9.9.9", WINDOWS, cache, base);

    assertEquals("fakes3.exe", exe.getFileName().toString());
    assertArrayEquals(BINARY, Files.readAllBytes(exe));
  }

  @Test
  void refusesAnArchiveWhoseChecksumDoesNotMatch() throws Exception {
    byte[] archive = tarGz("fakes3-x86_64-unknown-linux-musl/fakes3", BINARY);
    String wrong = sha256("something else".getBytes(StandardCharsets.UTF_8));
    publish(LINUX, archive, wrong);

    FakeS3Exception e = assertThrows(FakeS3Exception.class, () -> Binaries.download("9.9.9", LINUX, cache, base));

    assertTrue(e.getMessage().contains("SHA-256 mismatch"), e.getMessage());
    assertTrue(e.getMessage().contains("expected " + wrong), e.getMessage());
    assertTrue(e.getMessage().contains("got " + sha256(archive)), e.getMessage());
    Path dir = cache.resolve("9.9.9").resolve(LINUX.triple);
    assertFalse(Files.exists(dir.resolve("fakes3")), "nothing was installed");
    try (Stream<Path> left = Files.list(dir)) {
      assertEquals(0, left.filter(f -> f.toString().endsWith(".part")).count(), "temporary files are removed");
    }
  }

  @Test
  void refusesAChecksumFileWithoutAChecksum() throws Exception {
    byte[] archive = tarGz("fakes3-x86_64-unknown-linux-musl/fakes3", BINARY);
    publish(LINUX, archive, "<html>not found</html>");

    FakeS3Exception e = assertThrows(FakeS3Exception.class, () -> Binaries.download("9.9.9", LINUX, cache, base));
    assertTrue(e.getMessage().contains("no SHA-256 checksum"), e.getMessage());
  }

  @Test
  void reportsAMissingRelease() {
    FakeS3Exception e = assertThrows(FakeS3Exception.class, () -> Binaries.download("9.9.9", LINUX, cache, base));
    assertTrue(e.getMessage().contains("HTTP 404"), e.getMessage());
  }

  @Test
  void reportsAMissingConfiguredBinary() {
    Path nowhere = cache.resolve("no-such-fakes3");
    FakeS3Exception e = assertThrows(FakeS3Exception.class, () -> FakeS3.builder().binary(nowhere).start());
    assertTrue(e.getMessage().contains(nowhere.toString()), e.getMessage());
  }

  static String sha256(byte[] data) throws Exception {
    return Binaries.hex(MessageDigest.getInstance("SHA-256").digest(data));
  }

  /** A ustar archive of (name, content) pairs, gzipped. */
  static byte[] tarGz(Object... entries) throws IOException {
    ByteArrayOutputStream bytes = new ByteArrayOutputStream();
    try (GZIPOutputStream gz = new GZIPOutputStream(bytes)) {
      for (int i = 0; i < entries.length; i += 2) {
        String name = (String) entries[i];
        byte[] data = (byte[]) entries[i + 1];
        byte[] h = new byte[512];
        put(h, 0, name);
        put(h, 100, "0000755\0");
        put(h, 108, "0000000\0");
        put(h, 116, "0000000\0");
        put(h, 124, String.format("%011o\0", data.length));
        put(h, 136, String.format("%011o\0", 0));
        h[156] = '0';
        put(h, 257, "ustar\0");
        put(h, 263, "00");
        put(h, 148, "        ");
        int sum = 0;
        for (byte b : h) {
          sum += b & 0xff;
        }
        put(h, 148, String.format("%06o\0 ", sum));
        gz.write(h);
        gz.write(data);
        gz.write(new byte[(512 - data.length % 512) % 512]);
      }
      gz.write(new byte[1024]);
    }
    return bytes.toByteArray();
  }

  private static void put(byte[] h, int off, String s) {
    byte[] b = s.getBytes(StandardCharsets.US_ASCII);
    System.arraycopy(b, 0, h, off, b.length);
  }
}
