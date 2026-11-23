package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.time.Instant;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.presigner.S3Presigner;

@ExtendWith(FakeS3Extension.class)
class ClockTest {
  private static final HttpClient HTTP = HttpClient.newHttpClient();

  @Test
  void presignedUrlExpiresWhenTheClockMoves(FakeS3 s3) throws Exception {
    URI url;
    try (S3Client client = Clients.s3(s3);
        S3Presigner presigner = Clients.presigner(s3)) {
      client.createBucket(b -> b.bucket("docs"));
      client.putObject(b -> b.bucket("docs").key("report.pdf"), RequestBody.fromString("pdf"));
      url =
          presigner
              .presignGetObject(
                  p -> p.signatureDuration(Duration.ofSeconds(60)).getObjectRequest(g -> g.bucket("docs").key("report.pdf")))
              .url()
              .toURI();
    }

    HttpResponse<String> fresh = HTTP.send(HttpRequest.newBuilder(url).build(), HttpResponse.BodyHandlers.ofString());
    assertEquals(200, fresh.statusCode());
    assertEquals("pdf", fresh.body());

    s3.clock().advance(Duration.ofMinutes(2));

    HttpResponse<String> expired = HTTP.send(HttpRequest.newBuilder(url).build(), HttpResponse.BodyHandlers.ofString());
    assertEquals(403, expired.statusCode());
    assertTrue(expired.body().contains("expired"), expired.body());
  }

  @Test
  void freezeAdvanceReset(FakeS3 s3) {
    Instant at = Instant.parse("2030-01-02T03:04:05Z");
    assertEquals(at, s3.clock().freeze(at));
    assertTrue(s3.clock().isFrozen());
    assertEquals(Instant.parse("2030-02-02T03:04:05Z"), s3.clock().advance("31d"));
    assertEquals(Instant.parse("2030-02-02T04:04:05Z"), s3.clock().advance(Duration.ofHours(1)));
    assertEquals(Instant.parse("2030-02-02T04:04:05Z"), s3.clock().now());
    s3.clock().reset();
    assertFalse(s3.clock().isShifted());
  }
}
