package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import java.util.List;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.NoSuchKeyException;

@ExtendWith(FakeS3Extension.class)
class RequestLogTest {
  @Test
  void assertsOnWhatWasSent(FakeS3 s3) {
    try (S3Client client = Clients.s3(s3)) {
      client.createBucket(b -> b.bucket("logs"));
      for (String key : List.of("a/1.txt", "a/2.txt", "b/3.txt")) {
        client.putObject(b -> b.bucket("logs").key(key), RequestBody.fromString(key));
      }
      assertThrows(NoSuchKeyException.class, () -> client.getObject(b -> b.bucket("logs").key("missing")));
    }

    List<RecordedRequest> puts = s3.requests().list(RequestFilter.create().operation("PutObject"));
    assertEquals(
        List.of("a/1.txt", "a/2.txt", "b/3.txt"),
        puts.stream().map(RecordedRequest::key).collect(Collectors.toList()));
    RecordedRequest first = puts.get(0);
    assertEquals("PUT", first.method());
    assertEquals("logs", first.bucket());
    assertEquals(200, first.status());
    assertEquals(7, first.bytesIn());
    assertEquals(s3.accessKey(), first.accessKey());
    assertFalse(first.requestId().isEmpty());

    assertEquals(2, s3.requests().count(RequestFilter.create().operation("PutObject").keyPrefix("a/")));
    assertEquals(1, s3.requests().count(RequestFilter.create().status("error")));
    RecordedRequest miss = s3.requests().list(RequestFilter.create().status(404)).get(0);
    assertEquals("GetObject", miss.operation());

    List<RecordedRequest> last = s3.requests().list(RequestFilter.create().limit(1));
    assertEquals(miss.seq(), last.get(0).seq());
    assertEquals(0, s3.requests().list(RequestFilter.create().after(miss.seq())).size());

    s3.requests().clear();
    assertEquals(0, s3.requests().count());
    assertEquals(0, s3.requests().config().stored());

    s3.requests().setCapacity(5);
    RequestLogConfig off = s3.requests().setEnabled(false);
    assertFalse(off.enabled());
    assertEquals(5, off.capacity());
  }
}
