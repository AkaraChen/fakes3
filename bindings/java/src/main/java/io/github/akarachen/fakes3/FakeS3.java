package io.github.akarachen.fakes3;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.net.URI;
import java.net.URLEncoder;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Deque;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.OptionalLong;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * An in-memory S3 server for one test (or one test class), running as a child process.
 *
 * <pre>{@code
 * try (FakeS3 s3 = FakeS3.start()) {
 *   S3Client client = S3Client.builder()
 *       .endpointOverride(URI.create(s3.endpoint()))
 *       .forcePathStyle(true)
 *       .region(Region.US_EAST_1)
 *       .credentialsProvider(StaticCredentialsProvider.create(
 *           AwsBasicCredentials.create(s3.accessKey(), s3.secretKey())))
 *       .build();
 *   // ...
 * }
 * }</pre>
 *
 * <p>Every server has its own data, clock and port. {@link #close()} stops the process; servers
 * still running when the JVM exits are stopped by a shutdown hook.
 *
 * <p>Control calls ({@link #reset()}, snapshots, {@link #clock()}, {@link #requests()}) are not
 * synchronized with in-flight S3 requests: make them between tests, or while no request is pending.
 */
public final class FakeS3 implements AutoCloseable {
  private static final Set<Process> LIVE = ConcurrentHashMap.newKeySet();

  static {
    Runtime.getRuntime()
        .addShutdownHook(
            new Thread(
                () -> {
                  for (Process p : LIVE) {
                    p.destroyForcibly();
                  }
                },
                "fakes3-shutdown"));
  }

  private static volatile HttpClient sharedHttp;

  private final Process process;
  private final Path binary;
  private final String endpoint;
  private final URI control;
  private final String host;
  private final int port;
  private final String accessKey;
  private final String secretKey;
  private final Clock clock = new Clock(this);
  private final Requests requests = new Requests(this);
  private volatile boolean closed;

  private FakeS3(Process process, Path binary, String endpoint, String accessKey, String secretKey) {
    this.process = process;
    this.binary = binary;
    this.endpoint = endpoint;
    this.accessKey = accessKey;
    this.secretKey = secretKey;
    URI u = URI.create(endpoint);
    this.host = u.getHost();
    this.port = u.getPort();
    // A wildcard address isn't something to connect to; the control plane is on loopback too.
    String controlHost = host.equals("0.0.0.0") ? "127.0.0.1" : host.equals("[::]") ? "[::1]" : host;
    this.control = URI.create("http://" + controlHost + ":" + port + "/_fakes3/");
  }

  /** Starts a server with the default settings: {@code 127.0.0.1}, a free port. */
  public static FakeS3 start() {
    return builder().start();
  }

  /** Settings for {@link Builder#start()}; a builder can start any number of servers. */
  public static Builder builder() {
    return new Builder();
  }

  /** {@code http://host:port}, ready for an S3 client with path-style addressing. */
  public String endpoint() {
    return endpoint;
  }

  public String host() {
    return host;
  }

  public int port() {
    return port;
  }

  /** The default account's access key; any credentials work unless the server verifies signatures. */
  public String accessKey() {
    return accessKey;
  }

  public String secretKey() {
    return secretKey;
  }

  /** The executable this server runs. */
  public Path binary() {
    return binary;
  }

  /** The child process's id. */
  public long pid() {
    return process.pid();
  }

  public boolean isRunning() {
    return !closed && process.isAlive();
  }

  /** Stops the server and waits for the process to exit. Safe to call more than once. */
  @Override
  public void close() {
    if (closed) {
      return;
    }
    closed = true;
    stop(process);
  }

  /** Same as {@link #close()}. */
  public void stop() {
    close();
  }

  private static void stop(Process p) {
    try {
      p.destroy();
      if (!p.waitFor(5, TimeUnit.SECONDS)) {
        p.destroyForcibly();
        p.waitFor(5, TimeUnit.SECONDS);
      }
    } catch (InterruptedException e) {
      p.destroyForcibly();
      Thread.currentThread().interrupt();
    } finally {
      LIVE.remove(p);
    }
  }

  /**
   * Clears all buckets, objects and multipart uploads, the request log and the clock. Snapshots are
   * kept.
   */
  public void reset() {
    reset(ResetOptions.create());
  }

  public void reset(ResetOptions options) {
    call(
        "POST",
        "reset?clock=" + options.clock + "&requests=" + options.requests + "&snapshots=" + options.snapshots,
        null);
  }

  /** Saves the current state under a generated name ({@code snap-<n>}). */
  public Snapshot snapshot() {
    return Snapshot.from(call("POST", "snapshots", "{}"));
  }

  /**
   * Saves the current state: buckets, objects with all their versions, settings and multipart
   * uploads. Object data is shared, not copied. Names are 1 to 64 characters of {@code
   * A-Za-z0-9._-}; an existing name fails with code {@code Conflict}.
   */
  public Snapshot snapshot(String name) {
    return Snapshot.from(call("POST", "snapshots", "{\"name\":" + Json.quote(name) + "}"));
  }

  /** Replaces all data with the snapshot's. The snapshot stays and can be restored again. */
  public Snapshot restore(String name) {
    return Snapshot.from(call("POST", "snapshots/" + encode(name) + "/restore", null));
  }

  public List<Snapshot> snapshots() {
    List<Snapshot> out = new ArrayList<>();
    for (Object o : list(call("GET", "snapshots", null).get("snapshots"))) {
      out.add(Snapshot.from(map(o)));
    }
    return Collections.unmodifiableList(out);
  }

  public void deleteSnapshot(String name) {
    call("DELETE", "snapshots/" + encode(name), null);
  }

  /** The server clock: freeze it, move it, reset it. */
  public Clock clock() {
    return clock;
  }

  /** The log of S3 requests the server received. */
  public Requests requests() {
    return requests;
  }

  /** Bytes of object data held, and the configured limit. */
  public Memory memory() {
    Map<String, Object> m = map(call("GET", "stats", null).get("memory"));
    Object limit = m.get("limit");
    return new Memory(
        ((Number) m.get("used")).longValue(),
        limit == null ? OptionalLong.empty() : OptionalLong.of(((Number) limit).longValue()));
  }

  @Override
  public String toString() {
    return "FakeS3[" + endpoint + (closed ? ", stopped" : "") + "]";
  }

  // ---- control plane ----------------------------------------------------------------------------

  Map<String, Object> call(String method, String path, String body) {
    if (closed) {
      throw new IllegalStateException("server is stopped");
    }
    HttpRequest.Builder req =
        HttpRequest.newBuilder(control.resolve(path))
            .timeout(Duration.ofSeconds(30))
            .header("Content-Type", "application/json");
    req.method(
        method,
        body == null
            ? HttpRequest.BodyPublishers.noBody()
            : HttpRequest.BodyPublishers.ofString(body, StandardCharsets.UTF_8));
    HttpResponse<String> resp;
    try {
      resp = http().send(req.build(), HttpResponse.BodyHandlers.ofString(StandardCharsets.UTF_8));
    } catch (IOException e) {
      String why = process.isAlive() ? e.toString() : "the fakes3 process exited (" + process.exitValue() + ")";
      throw new FakeS3Exception("control call " + method + " /_fakes3/" + path + " failed: " + why, e);
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      throw new FakeS3Exception("interrupted", e);
    }
    String text = resp.body();
    Map<String, Object> json;
    try {
      json = text == null || text.isEmpty() ? new LinkedHashMap<>() : Json.parseObject(text);
    } catch (IllegalArgumentException e) {
      throw new FakeS3Exception(
          "unexpected response to " + method + " /_fakes3/" + path + " (HTTP " + resp.statusCode() + "): " + text);
    }
    if (resp.statusCode() >= 400) {
      Object err = json.get("error");
      String code = null;
      String message = text;
      if (err instanceof Map) {
        Map<String, Object> e = map(err);
        code = (String) e.get("code");
        message = (String) e.get("message");
      }
      throw new FakeS3Exception(message, code, resp.statusCode(), null);
    }
    return json;
  }

  private static HttpClient http() {
    HttpClient h = sharedHttp;
    if (h == null) {
      synchronized (FakeS3.class) {
        h = sharedHttp;
        if (h == null) {
          h =
              HttpClient.newBuilder()
                  .version(HttpClient.Version.HTTP_1_1)
                  .connectTimeout(Duration.ofSeconds(10))
                  .proxy(java.net.ProxySelector.of(null))
                  .build();
          sharedHttp = h;
        }
      }
    }
    return h;
  }

  static String encode(String s) {
    return URLEncoder.encode(s, StandardCharsets.UTF_8).replace("+", "%20");
  }

  @SuppressWarnings("unchecked")
  static Map<String, Object> map(Object o) {
    return o instanceof Map ? (Map<String, Object>) o : Collections.emptyMap();
  }

  @SuppressWarnings("unchecked")
  static List<Object> list(Object o) {
    return o instanceof List ? (List<Object>) o : Collections.emptyList();
  }

  static Instant instant(Object o) {
    return o == null ? null : Instant.parse((String) o);
  }

  static long number(Object o) {
    return o == null ? 0 : ((Number) o).longValue();
  }

  // ---- builder ----------------------------------------------------------------------------------

  /** Server settings. Every setting is optional. */
  public static final class Builder {
    private String host = "127.0.0.1";
    private int port = 0;
    private String accessKey = "fakes3";
    private String secretKey = "fakes3secret";
    private boolean auth;
    private final List<String> users = new ArrayList<>();
    private String bucketDefaults;
    private String memoryLimit;
    private final List<String> domains = new ArrayList<>();
    private boolean requestLog = true;
    private Integer requestLogCapacity;
    private final List<String> extraArgs = new ArrayList<>();
    private Duration startTimeout = Duration.ofSeconds(30);
    private final Binaries.Source source = new Binaries.Source();

    private Builder() {}

    /** Address to listen on; default {@code 127.0.0.1}. */
    public Builder host(String host) {
      this.host = host;
      return this;
    }

    /** Port to listen on; default 0, a free one. */
    public Builder port(int port) {
      this.port = port;
      return this;
    }

    /** Credentials of the default account; default {@code fakes3} / {@code fakes3secret}. */
    public Builder credentials(String accessKey, String secretKey) {
      this.accessKey = accessKey;
      this.secretKey = secretKey;
      return this;
    }

    /**
     * Verify request signatures. By default any credentials work: a signed request acts as the
     * account its access key belongs to, and an unsigned request is anonymous.
     */
    public Builder auth(boolean auth) {
      this.auth = auth;
      return this;
    }

    /** Another account, owning the buckets and objects it creates. */
    public Builder user(String name, String accessKey, String secretKey) {
      users.add(name + ":" + accessKey + ":" + secretKey);
      return this;
    }

    /** {@code "legacy"} (default) or {@code "aws"}: the settings new buckets start with. */
    public Builder bucketDefaults(String bucketDefaults) {
      this.bucketDefaults = bucketDefaults;
      return this;
    }

    /**
     * Cap on object data: bytes, {@code "512MiB"}, {@code "2GiB"}, {@code "unlimited"}, or {@code
     * "auto"} (default, 75% of the machine's or container's memory). Writes past it get HTTP 507.
     */
    public Builder memoryLimit(String memoryLimit) {
      this.memoryLimit = memoryLimit;
      return this;
    }

    /** A base domain for virtual-host addressing ({@code <bucket>.<domain>}); repeatable. */
    public Builder domain(String domain) {
      domains.add(domain);
      return this;
    }

    /** Whether to record requests for {@link FakeS3#requests()}; default true. */
    public Builder requestLog(boolean enabled) {
      this.requestLog = enabled;
      return this;
    }

    /** How many requests the log keeps; default 10000. */
    public Builder requestLogCapacity(int capacity) {
      this.requestLogCapacity = capacity;
      return this;
    }

    /** Extra command-line arguments for {@code fakes3}. */
    public Builder arg(String... args) {
      Collections.addAll(extraArgs, args);
      return this;
    }

    /** How long to wait for the server to report its address; default 30 seconds. */
    public Builder startTimeout(Duration timeout) {
      this.startTimeout = timeout;
      return this;
    }

    /**
     * The fakes3 executable to run. Without it: the {@code fakes3.binary} system property, the
     * {@code FAKES3_BINARY} environment variable, {@code fakes3} on PATH, then a download.
     */
    public Builder binary(Path binary) {
      source.binary = binary;
      return this;
    }

    /** The fakes3 release to download when no binary is configured or on PATH. */
    public Builder version(String version) {
      source.version = version;
      return this;
    }

    /** Where downloaded binaries are kept. */
    public Builder cacheDir(Path cacheDir) {
      source.cacheDir = cacheDir;
      return this;
    }

    /**
     * Base URL to download releases from (a mirror); the archive is fetched from {@code
     * <url>/v<version>/<archive>}, next to its {@code .sha256}.
     */
    public Builder downloadUrl(URI downloadUrl) {
      source.downloadUrl = downloadUrl;
      return this;
    }

    /** Starts a server and waits until it accepts connections. */
    public FakeS3 start() {
      Path bin = Binaries.resolve(source);
      List<String> cmd = new ArrayList<>();
      cmd.add(bin.toString());
      cmd.add("--host");
      cmd.add(host);
      cmd.add("--port");
      cmd.add(Integer.toString(port));
      cmd.add("--access-key");
      cmd.add(accessKey);
      cmd.add("--secret-key");
      cmd.add(secretKey);
      if (auth) {
        cmd.add("--auth");
      }
      for (String u : users) {
        cmd.add("--user");
        cmd.add(u);
      }
      if (bucketDefaults != null) {
        cmd.add("--bucket-defaults");
        cmd.add(bucketDefaults);
      }
      if (memoryLimit != null) {
        cmd.add("--memory-limit");
        cmd.add(memoryLimit);
      }
      for (String d : domains) {
        cmd.add("--domain");
        cmd.add(d);
      }
      if (!requestLog) {
        cmd.add("--no-request-log");
      }
      if (requestLogCapacity != null) {
        cmd.add("--request-log-capacity");
        cmd.add(requestLogCapacity.toString());
      }
      cmd.addAll(extraArgs);
      return launch(bin, cmd, accessKey, secretKey, startTimeout);
    }
  }

  private static FakeS3 launch(
      Path bin, List<String> cmd, String accessKey, String secretKey, Duration timeout) {
    ProcessBuilder pb = new ProcessBuilder(cmd);
    // The server reads FAKES3_* settings from its environment; only the arguments should count.
    pb.environment().keySet().removeIf(k -> k.toUpperCase(Locale.ROOT).startsWith("FAKES3_"));
    Process p;
    try {
      p = pb.start();
    } catch (IOException e) {
      throw new FakeS3Exception("cannot run " + bin + ": " + e.getMessage(), e);
    }
    LIVE.add(p);
    try {
      Deque<String> stderr = new ArrayDeque<>();
      Thread errDrain = drain(p.getErrorStream(), stderr, "fakes3-stderr-" + p.pid());
      CompletableFuture<String> first = new CompletableFuture<>();
      Thread outDrain =
          new Thread(
              () -> {
                try (BufferedReader r =
                    new BufferedReader(new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
                  String line;
                  while ((line = r.readLine()) != null) {
                    if (!first.isDone() && line.startsWith("http")) {
                      first.complete(line.trim());
                    }
                  }
                } catch (IOException ignored) {
                  // The process went away.
                }
                first.complete(null);
              },
              "fakes3-stdout-" + p.pid());
      outDrain.setDaemon(true);
      outDrain.start();

      String endpoint;
      try {
        endpoint = first.get(timeout.toMillis(), TimeUnit.MILLISECONDS);
      } catch (TimeoutException e) {
        throw new FakeS3Exception(
            bin + " did not report an address within " + timeout + tail(stderr));
      } catch (ExecutionException e) {
        throw new FakeS3Exception("cannot start " + bin, e.getCause());
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw new FakeS3Exception("interrupted while starting fakes3", e);
      }
      if (endpoint == null) {
        try {
          p.waitFor(2, TimeUnit.SECONDS);
          errDrain.join(1000);
        } catch (InterruptedException e) {
          Thread.currentThread().interrupt();
        }
        String status = p.isAlive() ? "closed its output" : "exited with status " + p.exitValue();
        throw new FakeS3Exception(bin + " " + status + " before it was ready" + tail(stderr));
      }
      return new FakeS3(p, bin, endpoint, accessKey, secretKey);
    } catch (RuntimeException | Error e) {
      stop(p);
      throw e;
    }
  }

  private static Thread drain(InputStream in, Deque<String> tail, String name) {
    Thread t =
        new Thread(
            () -> {
              try (BufferedReader r = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8))) {
                String line;
                while ((line = r.readLine()) != null) {
                  synchronized (tail) {
                    tail.addLast(line);
                    if (tail.size() > 50) {
                      tail.removeFirst();
                    }
                  }
                }
              } catch (IOException ignored) {
                // The process went away.
              }
            },
            name);
    t.setDaemon(true);
    t.start();
    return t;
  }

  private static String tail(Deque<String> lines) {
    synchronized (lines) {
      return lines.isEmpty() ? "" : "; stderr:\n" + String.join("\n", lines);
    }
  }
}
