package matrix;

import java.security.SecureRandom;
import java.util.*;

/** One SDK's implementation of the scenario list. */
abstract class Suite {
    @FunctionalInterface
    interface Body { void run() throws Exception; }

    /** Thrown when the SDK has no API for a scenario. */
    static final class Skip extends RuntimeException {
        Skip(String reason) { super(reason); }
    }

    final Env env;
    final String sdk;
    final String bucket;

    Suite(Env env, String sdk) {
        this.env = env;
        this.sdk = sdk;
        this.bucket = "m-" + sdk + "-" + randLower(8);
    }

    static String randLower(int n) {
        String a = "abcdefghijklmnopqrstuvwxyz0123456789";
        SecureRandom r = new SecureRandom();
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < n; i++) sb.append(a.charAt(r.nextInt(a.length())));
        return sb.toString();
    }

    abstract Map<String, Body> scenarios();

    /** HTTP status of an SDK service error, or -1. */
    abstract int status(Throwable t);

    /** S3 error code of an SDK service error, or null. */
    abstract String code(Throwable t);

    void cleanup() throws Exception {}

    /** Runs b, which must fail with one of the statuses or one of the codes. */
    void expectError(String what, Body b, Set<Integer> statuses, String... codes) throws Exception {
        try {
            b.run();
        } catch (Throwable e) {
            Throwable c = U.unwrap(e);
            if (c instanceof Skip s) throw s;
            int st = status(c);
            String cd = code(c);
            if (statuses.contains(st) || (cd != null && Arrays.asList(codes).contains(cd))) return;
            throw new AssertionError(what + ": expected " + statuses + "/" + Arrays.toString(codes)
                    + " but got status=" + st + " code=" + cd + " (" + U.describe(c) + ")");
        }
        throw new AssertionError(what + ": expected error " + statuses + "/" + Arrays.toString(codes) + " but call succeeded");
    }
}
