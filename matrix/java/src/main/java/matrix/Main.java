package matrix;

import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import java.util.concurrent.*;

public final class Main {
    /** Scenario IDs from matrix/SPEC.md, in spec order. */
    static final List<String> IDS = List.of(
            "create_bucket", "head_bucket", "head_bucket_missing", "list_buckets",
            "put_get_small", "put_get_empty", "put_get_large", "special_keys", "head_object_metadata",
            "get_range", "get_range_invalid", "get_missing_key", "get_if_none_match", "get_if_match_fail",
            "put_if_none_match", "delete_object", "delete_missing_object", "delete_objects",
            "list_v2_basic", "list_v2_prefix_delimiter", "list_v2_pagination", "list_v2_start_after", "list_v1",
            "copy_object", "copy_object_replace_metadata",
            "multipart_upload", "multipart_abort", "multipart_list_parts", "multipart_too_small",
            "high_level_upload", "presigned_get", "presigned_put",
            "checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme",
            "content_md5_bad", "bad_credentials", "delete_bucket_not_empty",
            "versioning", "object_tagging", "delete_bucket");

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: matrix <aws-java-v2|aws-java-v2-crt|aws-java-v1|minio-java>");
            System.exit(2);
        }
        String sdk = args[0];
        Env env = Env.fromSystem();
        Suite suite = switch (sdk) {
            case "aws-java-v2" -> new V2Suite(env, sdk, false);
            case "aws-java-v2-crt" -> new V2Suite(env, sdk, true);
            case "aws-java-v1" -> new V1Suite(env, sdk);
            case "minio-java" -> new MinioSuite(env, sdk);
            default -> throw new IllegalArgumentException("unknown sdk " + sdk);
        };
        Map<String, Suite.Body> scenarios = suite.scenarios();
        ExecutorService pool = Executors.newCachedThreadPool(r -> {
            Thread t = new Thread(r);
            t.setDaemon(true);
            return t;
        });
        // Dev aid: MATRIX_ONLY=id1,id2 runs a subset (results file then only holds that subset).
        String only = System.getenv("MATRIX_ONLY");
        Set<String> onlySet = only == null || only.isBlank() ? null : Set.of(only.split(","));
        List<String> rows = new ArrayList<>();
        int pass = 0, fail = 0, skip = 0;
        for (String id : IDS) {
            if (onlySet != null && !onlySet.contains(id)) continue;
            Suite.Body body = scenarios.get(id);
            String status;
            String error = null;
            long t0 = System.nanoTime();
            if (body == null) {
                status = "fail";
                error = "scenario not implemented in harness";
            } else {
                Future<?> f = pool.submit(() -> { body.run(); return null; });
                try {
                    f.get(60, TimeUnit.SECONDS);
                    status = "pass";
                } catch (TimeoutException e) {
                    f.cancel(true);
                    status = "fail";
                    error = "Timeout: scenario exceeded 60s";
                } catch (ExecutionException e) {
                    Throwable c = U.unwrap(e);
                    if (c instanceof Suite.Skip s) {
                        status = "skip";
                        error = s.getMessage();
                    } else {
                        status = "fail";
                        error = U.describe(c);
                    }
                }
            }
            double ms = (System.nanoTime() - t0) / 1e6;
            switch (status) { case "pass" -> pass++; case "fail" -> fail++; default -> skip++; }
            StringBuilder sb = new StringBuilder("    {\"scenario\": ").append(U.json(id))
                    .append(", \"status\": ").append(U.json(status));
            if (!status.equals("skip")) sb.append(", \"ms\": ").append(String.format(Locale.ROOT, "%.1f", ms));
            if (error != null) sb.append(", \"error\": ").append(U.json(U.trunc(error)));
            sb.append("}");
            rows.add(sb.toString());
            System.err.printf("[%s] %-30s %s%s%n", sdk, id, status, error == null ? "" : "  " + U.trunc(error));
        }
        try {
            Future<?> f = pool.submit(() -> { suite.cleanup(); return null; });
            f.get(120, TimeUnit.SECONDS);
        } catch (Exception e) {
            System.err.println("[" + sdk + "] cleanup failed: " + U.describe(U.unwrap(e)));
        }
        String version;
        try (InputStream in = Main.class.getResourceAsStream("/versions.properties")) {
            Properties p = new Properties();
            p.load(in);
            version = p.getProperty(sdk, "unknown");
        }
        String json = "{\n  \"sdk\": " + U.json(sdk) + ",\n  \"lang\": \"java\",\n  \"version\": " + U.json(version)
                + ",\n  \"server\": " + U.json(env.server()) + ",\n  \"results\": [\n" + String.join(",\n", rows) + "\n  ]\n}\n";
        Path out = Path.of(env.out());
        Files.createDirectories(out);
        Files.writeString(out.resolve(sdk + ".json"), json, StandardCharsets.UTF_8);
        System.err.printf("[%s] pass=%d fail=%d skip=%d -> %s%n", sdk, pass, fail, skip, out.resolve(sdk + ".json"));
        System.exit(0);
    }
}
