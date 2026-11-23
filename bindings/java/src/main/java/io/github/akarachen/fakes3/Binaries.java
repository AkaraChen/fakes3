package io.github.akarachen.fakes3;

import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.ProxySelector;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.channels.FileChannel;
import java.nio.channels.FileLock;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.nio.file.StandardOpenOption;
import java.security.DigestInputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.time.Duration;
import java.util.Locale;
import java.util.Properties;
import java.util.zip.GZIPInputStream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

/** Finds the fakes3 executable: a configured path, then PATH, then a verified download. */
final class Binaries {
  static final String DEFAULT_DOWNLOAD_URL = "https://github.com/AkaraChen/fakes3/releases/download";

  private Binaries() {}

  /** Settings that decide where the binary comes from; nulls fall back to properties and env. */
  static final class Source {
    Path binary;
    String version;
    Path cacheDir;
    URI downloadUrl;
  }

  static Path resolve(Source src) {
    Path explicit = src.binary;
    if (explicit == null) {
      String p = setting("fakes3.binary", "FAKES3_BINARY");
      if (p != null) {
        explicit = Paths.get(p);
      }
    }
    if (explicit != null) {
      if (!Files.isRegularFile(explicit)) {
        throw new FakeS3Exception("fakes3 binary not found at " + explicit);
      }
      return explicit;
    }
    Path onPath = findOnPath();
    if (onPath != null) {
      return onPath;
    }
    String version = src.version != null ? src.version : setting("fakes3.version", "FAKES3_VERSION");
    if (version == null) {
      version = bundledVersion();
    }
    if (version.startsWith("v")) {
      version = version.substring(1);
    }
    Path cache = src.cacheDir;
    if (cache == null) {
      String c = setting("fakes3.cacheDir", "FAKES3_CACHE_DIR");
      cache = c != null ? Paths.get(c) : defaultCacheDir();
    }
    URI base = src.downloadUrl;
    if (base == null) {
      String u = setting("fakes3.downloadUrl", "FAKES3_DOWNLOAD_URL");
      base = URI.create(u != null ? u : DEFAULT_DOWNLOAD_URL);
    }
    return download(version, Platform.current(), cache, base);
  }

  static String setting(String property, String env) {
    String v = System.getProperty(property);
    if (v == null || v.isEmpty()) {
      v = System.getenv(env);
    }
    return v == null || v.isEmpty() ? null : v;
  }

  static String bundledVersion() {
    try (InputStream in = Binaries.class.getResourceAsStream("fakes3.properties")) {
      Properties p = new Properties();
      if (in != null) {
        p.load(in);
      }
      String v = p.getProperty("binaryVersion");
      if (v == null || v.isEmpty() || v.startsWith("${")) {
        throw new FakeS3Exception("fakes3.properties is missing from the fakes3 jar");
      }
      return v;
    } catch (IOException e) {
      throw new FakeS3Exception("cannot read fakes3.properties", e);
    }
  }

  static Path findOnPath() {
    String path = System.getenv("PATH");
    if (path == null) {
      return null;
    }
    String exe = Platform.isWindows() ? "fakes3.exe" : "fakes3";
    for (String dir : path.split(java.io.File.pathSeparator)) {
      if (dir.isEmpty()) {
        continue;
      }
      try {
        Path candidate = Paths.get(dir, exe);
        if (Files.isRegularFile(candidate) && Files.isExecutable(candidate)) {
          return candidate;
        }
      } catch (RuntimeException ignored) {
        // A malformed PATH entry; skip it.
      }
    }
    return null;
  }

  static Path defaultCacheDir() {
    String home = System.getProperty("user.home");
    if (Platform.isWindows()) {
      String local = System.getenv("LOCALAPPDATA");
      Path base = local != null && !local.isEmpty() ? Paths.get(local) : Paths.get(home, "AppData", "Local");
      return base.resolve("fakes3").resolve("cache");
    }
    if (Platform.isMac()) {
      return Paths.get(home, "Library", "Caches", "fakes3");
    }
    String xdg = System.getenv("XDG_CACHE_HOME");
    Path base = xdg != null && !xdg.isEmpty() ? Paths.get(xdg) : Paths.get(home, ".cache");
    return base.resolve("fakes3");
  }

  private static final Object LOCK = new Object();

  /**
   * Returns {@code <cache>/<version>/<triple>/fakes3[.exe]}, downloading and verifying the release
   * archive first if it isn't there. Safe to call from several threads and JVMs at once.
   */
  static Path download(String version, Platform platform, Path cache, URI base) {
    Path dir = cache.resolve(version).resolve(platform.triple);
    Path exe = dir.resolve(platform.exe);
    if (Files.isRegularFile(exe)) {
      return exe;
    }
    synchronized (LOCK) {
      try {
        Files.createDirectories(dir);
        try (FileChannel ch =
                FileChannel.open(dir.resolve(".lock"), StandardOpenOption.CREATE, StandardOpenOption.WRITE);
            FileLock ignored = ch.lock()) {
          if (Files.isRegularFile(exe)) {
            return exe;
          }
          fetch(version, platform, dir, exe, base);
          return exe;
        }
      } catch (IOException e) {
        throw new FakeS3Exception("cannot install fakes3 " + version + " into " + dir + ": " + e, e);
      }
    }
  }

  private static void fetch(String version, Platform platform, Path dir, Path exe, URI base)
      throws IOException {
    String b = base.toString();
    if (!b.endsWith("/")) {
      b += "/";
    }
    URI archiveUrl = URI.create(b + "v" + version + "/" + platform.archive);
    URI sumUrl = URI.create(archiveUrl + ".sha256");
    HttpClient http =
        HttpClient.newBuilder()
            .followRedirects(HttpClient.Redirect.NORMAL)
            .connectTimeout(Duration.ofSeconds(30))
            .proxy(ProxySelector.getDefault())
            .build();

    String expected = parseSha256(new String(get(http, sumUrl), StandardCharsets.UTF_8), sumUrl);

    Path archive = Files.createTempFile(dir, platform.archive, ".part");
    Path extracted = Files.createTempFile(dir, platform.exe, ".part");
    try {
      String actual;
      try (InputStream body = stream(http, archiveUrl);
          DigestInputStream in = new DigestInputStream(body, sha256());
          OutputStream out = Files.newOutputStream(archive)) {
        in.transferTo(out);
        actual = hex(in.getMessageDigest().digest());
      }
      if (!actual.equals(expected)) {
        throw new FakeS3Exception(
            "SHA-256 mismatch for "
                + archiveUrl
                + ": expected "
                + expected
                + " (from "
                + sumUrl
                + "), got "
                + actual
                + ". The download is corrupt or was tampered with; nothing was installed.");
      }
      try (InputStream in = Files.newInputStream(archive)) {
        boolean found =
            platform.archive.endsWith(".zip")
                ? extractZip(in, platform.exe, extracted)
                : extractTarGz(in, platform.exe, extracted);
        if (!found) {
          throw new FakeS3Exception(archiveUrl + " does not contain " + platform.exe);
        }
      }
      if (!Platform.isWindows()) {
        extracted.toFile().setExecutable(true, false);
      }
      try {
        Files.move(extracted, exe, StandardCopyOption.ATOMIC_MOVE);
      } catch (AtomicMoveNotSupportedException e) {
        Files.move(extracted, exe, StandardCopyOption.REPLACE_EXISTING);
      }
    } finally {
      Files.deleteIfExists(archive);
      Files.deleteIfExists(extracted);
    }
  }

  static String parseSha256(String text, URI from) {
    String first = text.trim().split("\\s+", 2)[0].toLowerCase(Locale.ROOT);
    if (!first.matches("[0-9a-f]{64}")) {
      throw new FakeS3Exception("no SHA-256 checksum in " + from);
    }
    return first;
  }

  private static byte[] get(HttpClient http, URI url) throws IOException {
    try (InputStream in = stream(http, url)) {
      return in.readAllBytes();
    }
  }

  private static InputStream stream(HttpClient http, URI url) throws IOException {
    HttpRequest req =
        HttpRequest.newBuilder(url)
            .timeout(Duration.ofMinutes(5))
            .header("User-Agent", "fakes3-java")
            .GET()
            .build();
    HttpResponse<InputStream> resp;
    try {
      resp = http.send(req, HttpResponse.BodyHandlers.ofInputStream());
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new IOException("interrupted while downloading " + url, e);
    } catch (IOException e) {
      throw new IOException("cannot download " + url + ": " + e, e);
    }
    if (resp.statusCode() != 200) {
      resp.body().close();
      throw new FakeS3Exception("cannot download " + url + ": HTTP " + resp.statusCode());
    }
    return resp.body();
  }

  /** Copies the first regular file named {@code name} (in any directory) to {@code to}. */
  static boolean extractTarGz(InputStream raw, String name, Path to) throws IOException {
    InputStream in = new GZIPInputStream(raw);
    byte[] header = new byte[512];
    String longName = null;
    while (true) {
      if (!readBlock(in, header)) {
        return false;
      }
      if (isZero(header)) {
        return false;
      }
      String entryName = cString(header, 0, 100);
      if (header[257] == 'u' && header[258] == 's' && header[259] == 't') {
        String prefix = cString(header, 345, 155);
        if (!prefix.isEmpty()) {
          entryName = prefix + "/" + entryName;
        }
      }
      long size = octal(header, 124, 12);
      char type = (char) header[156];
      if (type == 'x' || type == 'L') {
        byte[] data = readFully(in, size);
        skipPadding(in, size);
        String meta = new String(data, StandardCharsets.UTF_8);
        longName = type == 'L' ? meta.replace("\0", "") : paxPath(meta, longName);
        continue;
      }
      if (longName != null) {
        entryName = longName;
        longName = null;
      }
      boolean regular = type == '0' || type == 0;
      if (regular && baseName(entryName).equals(name)) {
        try (OutputStream out = Files.newOutputStream(to)) {
          copy(in, out, size);
        }
        return true;
      }
      skip(in, size);
      skipPadding(in, size);
    }
  }

  static boolean extractZip(InputStream raw, String name, Path to) throws IOException {
    try (ZipInputStream zip = new ZipInputStream(raw)) {
      ZipEntry e;
      while ((e = zip.getNextEntry()) != null) {
        if (!e.isDirectory() && baseName(e.getName()).equals(name)) {
          Files.copy(zip, to, StandardCopyOption.REPLACE_EXISTING);
          return true;
        }
      }
    }
    return false;
  }

  private static String paxPath(String records, String current) {
    for (String line : records.split("\n")) {
      int sp = line.indexOf(' ');
      if (sp < 0) {
        continue;
      }
      String kv = line.substring(sp + 1);
      if (kv.startsWith("path=")) {
        return kv.substring(5);
      }
    }
    return current;
  }

  private static String baseName(String path) {
    String p = path.replace('\\', '/');
    while (p.endsWith("/")) {
      p = p.substring(0, p.length() - 1);
    }
    int slash = p.lastIndexOf('/');
    return slash < 0 ? p : p.substring(slash + 1);
  }

  private static boolean readBlock(InputStream in, byte[] block) throws IOException {
    int n = 0;
    while (n < block.length) {
      int r = in.read(block, n, block.length - n);
      if (r < 0) {
        if (n == 0) {
          return false;
        }
        throw new EOFException("truncated tar archive");
      }
      n += r;
    }
    return true;
  }

  private static boolean isZero(byte[] b) {
    for (byte x : b) {
      if (x != 0) {
        return false;
      }
    }
    return true;
  }

  private static String cString(byte[] b, int off, int len) {
    int end = off;
    while (end < off + len && b[end] != 0) {
      end++;
    }
    return new String(b, off, end - off, StandardCharsets.UTF_8);
  }

  private static long octal(byte[] b, int off, int len) {
    long v = 0;
    for (int k = off; k < off + len; k++) {
      byte c = b[k];
      if (c == 0 || c == ' ') {
        if (v != 0) {
          break;
        }
        continue;
      }
      if (c < '0' || c > '7') {
        throw new FakeS3Exception("unsupported tar header");
      }
      v = v * 8 + (c - '0');
    }
    return v;
  }

  private static byte[] readFully(InputStream in, long size) throws IOException {
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    copy(in, out, size);
    return out.toByteArray();
  }

  private static void copy(InputStream in, OutputStream out, long size) throws IOException {
    byte[] buf = new byte[64 * 1024];
    long left = size;
    while (left > 0) {
      int r = in.read(buf, 0, (int) Math.min(buf.length, left));
      if (r < 0) {
        throw new EOFException("truncated tar archive");
      }
      out.write(buf, 0, r);
      left -= r;
    }
  }

  private static void skip(InputStream in, long n) throws IOException {
    copy(in, OutputStream.nullOutputStream(), n);
  }

  private static void skipPadding(InputStream in, long size) throws IOException {
    long rem = size % 512;
    if (rem != 0) {
      skip(in, 512 - rem);
    }
  }

  private static MessageDigest sha256() {
    try {
      return MessageDigest.getInstance("SHA-256");
    } catch (NoSuchAlgorithmException e) {
      throw new IllegalStateException(e);
    }
  }

  static String hex(byte[] b) {
    StringBuilder s = new StringBuilder(b.length * 2);
    for (byte x : b) {
      s.append(Character.forDigit((x >> 4) & 0xf, 16)).append(Character.forDigit(x & 0xf, 16));
    }
    return s.toString();
  }

  /** A release target: its Rust triple, archive name and executable name. */
  static final class Platform {
    final String triple;
    final String archive;
    final String exe;

    Platform(String triple) {
      this.triple = triple;
      boolean windows = triple.contains("windows");
      this.archive = "fakes3-" + triple + (windows ? ".zip" : ".tar.gz");
      this.exe = windows ? "fakes3.exe" : "fakes3";
    }

    static boolean isWindows() {
      return os().startsWith("windows");
    }

    static boolean isMac() {
      String os = os();
      return os.startsWith("mac") || os.startsWith("darwin");
    }

    private static String os() {
      return System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
    }

    static Platform current() {
      String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);
      boolean arm = arch.equals("aarch64") || arch.equals("arm64");
      boolean x64 = arch.equals("amd64") || arch.equals("x86_64") || arch.equals("x64");
      if (isWindows()) {
        // Only an x86_64 build is published; Windows on Arm runs it under emulation.
        if (arm || x64) {
          return new Platform("x86_64-pc-windows-msvc");
        }
      } else if (isMac()) {
        if (arm) {
          return new Platform("aarch64-apple-darwin");
        }
        if (x64) {
          return new Platform("x86_64-apple-darwin");
        }
      } else if (os().startsWith("linux")) {
        // Static musl builds, so they run on any Linux distribution.
        if (arm) {
          return new Platform("aarch64-unknown-linux-musl");
        }
        if (x64) {
          return new Platform("x86_64-unknown-linux-musl");
        }
      }
      throw new FakeS3Exception(
          "no prebuilt fakes3 for "
              + System.getProperty("os.name")
              + "/"
              + arch
              + "; install it with `cargo install fakes3 --locked` and set -Dfakes3.binary or FAKES3_BINARY");
    }
  }
}
