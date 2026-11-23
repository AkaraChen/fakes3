package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertSame;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import software.amazon.awssdk.services.s3.S3Client;

/** A FakeS3 parameter on @BeforeAll starts one server for the class. */
@ExtendWith(FakeS3Extension.class)
class PerClassTest {
  static FakeS3 shared;

  @BeforeAll
  static void startOnce(FakeS3 s3) {
    shared = s3;
  }

  @BeforeEach
  void clean(FakeS3 s3) {
    s3.reset();
  }

  @Test
  void one(FakeS3 s3) {
    assertSame(shared, s3);
    createAndCount(s3);
  }

  @Test
  void two(FakeS3 s3) {
    assertSame(shared, s3);
    createAndCount(s3);
  }

  private static void createAndCount(FakeS3 s3) {
    try (S3Client client = Clients.s3(s3)) {
      client.createBucket(b -> b.bucket("shared"));
      assertEquals(1, client.listBuckets().buckets().size());
    }
  }
}
