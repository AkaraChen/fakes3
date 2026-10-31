package matrix;

public record Env(String endpoint, String accessKey, String secretKey, String region, String server, String out) {
    static Env fromSystem() {
        String server = get("MATRIX_SERVER", "fakes3");
        return new Env(get("S3_ENDPOINT", "http://127.0.0.1:9100"), get("S3_ACCESS_KEY", "fakes3"),
                get("S3_SECRET_KEY", "fakes3secret"), get("S3_REGION", "us-east-1"), server,
                get("MATRIX_OUT", "../results/" + server));
    }

    private static String get(String k, String def) {
        String v = System.getenv(k);
        return v == null || v.isEmpty() ? def : v;
    }
}
