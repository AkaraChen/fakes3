package io.github.akarachen.fakes3;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.akarachen.fakes3.junit.FakeS3Extension;
import java.util.List;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import software.amazon.awssdk.core.sync.RequestBody;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.S3Object;

@ExtendWith(FakeS3Extension.class)
class SnapshotTest {
  @Test
  void restoreRollsBack(FakeS3 s3) {
    try (S3Client client = Clients.s3(s3)) {
      client.createBucket(b -> b.bucket("fixtures"));
      client.putObject(b -> b.bucket("fixtures").key("seed.json"), RequestBody.fromString("{}"));

      Snapshot seeded = s3.snapshot("seeded");
      assertEquals("seeded", seeded.name());
      assertEquals(1, seeded.buckets());
      assertEquals(1, seeded.objects());

      client.putObject(b -> b.bucket("fixtures").key("added.json"), RequestBody.fromString("{}"));
      client.deleteObject(b -> b.bucket("fixtures").key("seed.json"));
      client.createBucket(b -> b.bucket("other"));
      assertEquals(List.of("added.json"), keys(client));

      s3.restore("seeded");
      assertEquals(List.of("seed.json"), keys(client));
      assertEquals(1, client.listBuckets().buckets().size());

      // A snapshot can be restored again and again.
      client.deleteObject(b -> b.bucket("fixtures").key("seed.json"));
      s3.restore("seeded");
      assertEquals(List.of("seed.json"), keys(client));

      // reset() clears the data but keeps snapshots.
      s3.reset();
      assertEquals(0, client.listBuckets().buckets().size());
      s3.restore("seeded");
      assertEquals(List.of("seed.json"), keys(client));
    }

    Snapshot auto = s3.snapshot();
    assertEquals(
        List.of("seeded", auto.name()),
        s3.snapshots().stream().map(Snapshot::name).collect(Collectors.toList()));
    s3.deleteSnapshot("seeded");
    s3.reset(ResetOptions.create().snapshots(true));
    assertEquals(List.of(), s3.snapshots());

    Memory memory = s3.memory();
    assertEquals(0, memory.used());
    assertTrue(memory.limit().isPresent(), "the default limit is 75% of memory");

    FakeS3Exception e = assertThrows(FakeS3Exception.class, () -> s3.restore("seeded"));
    assertEquals("NotFound", e.code());
    assertEquals(404, e.status());
  }

  private static List<String> keys(S3Client client) {
    return client.listObjectsV2(b -> b.bucket("fixtures")).contents().stream()
        .map(S3Object::key)
        .collect(Collectors.toList());
  }
}
