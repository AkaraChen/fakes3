package matrix;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.util.*;
import java.util.concurrent.CompletionException;
import java.util.concurrent.ExecutionException;
import java.util.zip.CRC32;
import java.util.zip.CRC32C;

final class U {
    static final int MIB = 1024 * 1024;
    static final HttpClient HTTP = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(10)).build();

    static Throwable unwrap(Throwable t) {
        while ((t instanceof ExecutionException || t instanceof CompletionException
                || t instanceof java.lang.reflect.UndeclaredThrowableException) && t.getCause() != null) {
            t = t.getCause();
        }
        return t;
    }

    static String describe(Throwable t) {
        String m = t.getMessage();
        if (m == null) m = "";
        int nl = m.indexOf('\n');
        if (nl >= 0) m = m.substring(0, nl);
        return trunc(t.getClass().getSimpleName() + (m.isEmpty() ? "" : ": " + m.strip()));
    }

    static String trunc(String s) { return s.length() > 300 ? s.substring(0, 300) : s; }

    static String json(String s) {
        StringBuilder sb = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            switch (c) {
                case '"' -> sb.append("\\\"");
                case '\\' -> sb.append("\\\\");
                case '\n' -> sb.append("\\n");
                case '\r' -> sb.append("\\r");
                case '\t' -> sb.append("\\t");
                default -> {
                    if (c < 0x20) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
                }
            }
        }
        return sb.append('"').toString();
    }

    static byte[] rand(int n) {
        byte[] b = new byte[n];
        new Random().nextBytes(b);
        return b;
    }

    static byte[] utf8(String s) { return s.getBytes(StandardCharsets.UTF_8); }

    static String str(byte[] b) { return new String(b, StandardCharsets.UTF_8); }

    static byte[] digest(String alg, byte[] data) {
        try {
            return MessageDigest.getInstance(alg).digest(data);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    static String hex(byte[] b) { return HexFormat.of().formatHex(b); }

    static String b64(byte[] b) { return Base64.getEncoder().encodeToString(b); }

    static String md5hex(byte[] data) { return hex(digest("MD5", data)); }

    static String md5b64(byte[] data) { return b64(digest("MD5", data)); }

    static String unquote(String etag) {
        if (etag == null) return null;
        return etag.replace("\"", "").replace("&quot;", "");
    }

    private static byte[] be(long v, int n) {
        byte[] out = new byte[n];
        for (int i = n - 1; i >= 0; i--) { out[i] = (byte) v; v >>>= 8; }
        return out;
    }

    private static final long[] CRC64NVME_TABLE = new long[256];
    static {
        long poly = 0x9a6c9329ac4bc9b5L; // reflected 0xad93d23594c93659
        for (int i = 0; i < 256; i++) {
            long c = i;
            for (int k = 0; k < 8; k++) c = (c & 1) != 0 ? (c >>> 1) ^ poly : c >>> 1;
            CRC64NVME_TABLE[i] = c;
        }
    }

    /** Expected base64 x-amz-checksum-* value. alg: CRC32, CRC32C, SHA1, SHA256, CRC64NVME. */
    static String checksum(String alg, byte[] data) {
        switch (alg) {
            case "CRC32" -> { CRC32 c = new CRC32(); c.update(data); return b64(be(c.getValue(), 4)); }
            case "CRC32C" -> { CRC32C c = new CRC32C(); c.update(data); return b64(be(c.getValue(), 4)); }
            case "SHA1" -> { return b64(digest("SHA-1", data)); }
            case "SHA256" -> { return b64(digest("SHA-256", data)); }
            case "CRC64NVME" -> {
                long crc = ~0L;
                for (byte b : data) crc = CRC64NVME_TABLE[(int) ((crc ^ b) & 0xff)] ^ (crc >>> 8);
                return b64(be(~crc, 8));
            }
            default -> throw new IllegalArgumentException(alg);
        }
    }

    static void check(boolean cond, String msg) {
        if (!cond) throw new AssertionError(msg);
    }

    static void eq(Object expected, Object actual, String what) {
        if (!Objects.equals(expected, actual)) throw new AssertionError(what + ": expected <" + expected + "> got <" + actual + ">");
    }

    static void bytesEq(byte[] expected, byte[] actual, String what) {
        if (!Arrays.equals(expected, actual)) {
            throw new AssertionError(what + ": content mismatch (expected " + expected.length + " bytes, got "
                    + (actual == null ? "null" : actual.length + " bytes") + ")");
        }
    }

    static byte[] httpGet(String url) throws Exception {
        HttpResponse<byte[]> r = HTTP.send(HttpRequest.newBuilder(URI.create(url)).GET().timeout(Duration.ofSeconds(30)).build(),
                HttpResponse.BodyHandlers.ofByteArray());
        if (r.statusCode() != 200) throw new AssertionError("presigned GET status " + r.statusCode() + ": " + trunc(str(r.body())));
        return r.body();
    }

    static void httpPut(String url, byte[] body) throws Exception {
        HttpResponse<byte[]> r = HTTP.send(HttpRequest.newBuilder(URI.create(url)).PUT(HttpRequest.BodyPublishers.ofByteArray(body))
                .timeout(Duration.ofSeconds(30)).build(), HttpResponse.BodyHandlers.ofByteArray());
        if (r.statusCode() != 200) throw new AssertionError("presigned PUT status " + r.statusCode() + ": " + trunc(str(r.body())));
    }

    static final String[] SPECIAL_KEYS = {"sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"};
    static final String[] CHECKSUM_ALGS = {"CRC32", "CRC32C", "SHA1", "SHA256", "CRC64NVME"};
    static final Map<String, String> USER_META = Map.of("color", "Blue", "x-y", "z");
}
