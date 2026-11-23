package io.github.akarachen.fakes3;

import java.net.URI;
import software.amazon.awssdk.auth.credentials.AwsBasicCredentials;
import software.amazon.awssdk.auth.credentials.StaticCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.S3Configuration;
import software.amazon.awssdk.services.s3.presigner.S3Presigner;

final class Clients {
  private Clients() {}

  static StaticCredentialsProvider credentials(FakeS3 s3) {
    return StaticCredentialsProvider.create(AwsBasicCredentials.create(s3.accessKey(), s3.secretKey()));
  }

  static S3Client s3(FakeS3 s3) {
    return S3Client.builder()
        .endpointOverride(URI.create(s3.endpoint()))
        .forcePathStyle(true)
        .region(Region.US_EAST_1)
        .credentialsProvider(credentials(s3))
        .build();
  }

  static S3Presigner presigner(FakeS3 s3) {
    return S3Presigner.builder()
        .endpointOverride(URI.create(s3.endpoint()))
        .serviceConfiguration(S3Configuration.builder().pathStyleAccessEnabled(true).build())
        .region(Region.US_EAST_1)
        .credentialsProvider(credentials(s3))
        .build();
  }
}
