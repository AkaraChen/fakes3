package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import java.util.List;
import java.util.stream.Collectors;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.junit.jupiter.api.extension.ExtendWith;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.Bucket;

/** Each test gets a server of its own: same bucket name, no leftovers, different ports. */
@ExtendWith(FakeS3Extension.class)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class IsolationTest {
  private static FakeS3 first;
  private FakeS3 fromBeforeEach;

  @BeforeEach
  void setUp(FakeS3 s3) {
    fromBeforeEach = s3;
  }

  @Test
  @Order(1)
  void firstTest(FakeS3 s3) {
    assertTrue(s3 == fromBeforeEach, "@BeforeEach and the test share one server");
    first = s3;
    useBucket(s3);
  }

  @Test
  @Order(2)
  void secondTest(FakeS3 s3) {
    assertNotEquals(first.endpoint(), s3.endpoint());
    assertTrue(!first.isRunning(), "the first test's server was stopped");
    useBucket(s3);
  }

  private static void useBucket(FakeS3 s3) {
    try (S3Client client = Clients.s3(s3)) {
      assertEquals(List.of(), client.listBuckets().buckets(), "starts empty");
      client.createBucket(b -> b.bucket("demo"));
      client.putObject(b -> b.bucket("demo").key("hello.txt"), RequestBody.fromString("hi"));
      List<String> names = client.listBuckets().buckets().stream().map(Bucket::name).collect(Collectors.toList());
      assertEquals(List.of("demo"), names);
    }
  }
}
