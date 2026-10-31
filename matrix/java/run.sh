#!/usr/bin/env bash
# Java SDK compatibility runner (see ../SPEC.md).
# Builds and runs inside the maven:3-eclipse-temurin-21 Docker image (no host JDK needed).
# Maven repo is cached in matrix/java/.m2; the shaded jar is rebuilt only when sources change.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${MATRIX_JAVA_IMAGE:-maven:3-eclipse-temurin-21}"
SDKS=(${MATRIX_JAVA_SDKS:-aws-java-v2 aws-java-v2-crt aws-java-v1 minio-java})

export S3_ENDPOINT="${S3_ENDPOINT:-http://127.0.0.1:9100}"
export S3_ACCESS_KEY="${S3_ACCESS_KEY:-fakes3}"
export S3_SECRET_KEY="${S3_SECRET_KEY:-fakes3secret}"
export S3_REGION="${S3_REGION:-us-east-1}"
export MATRIX_SERVER="${MATRIX_SERVER:-fakes3}"
MATRIX_OUT="${MATRIX_OUT:-$HERE/../results/$MATRIX_SERVER}"
mkdir -p "$MATRIX_OUT"
MATRIX_OUT="$(cd "$MATRIX_OUT" && pwd)"
export MATRIX_OUT

mkdir -p "$HERE/.m2" "$HERE/.home"
docker_run() {
  docker run --rm --network host --entrypoint "" \
    --user "$(id -u):$(id -g)" \
    -e HOME=/work/.home \
    -e S3_ENDPOINT -e S3_ACCESS_KEY -e S3_SECRET_KEY -e S3_REGION -e MATRIX_SERVER -e MATRIX_OUT -e MATRIX_ONLY \
    -v "$HERE:/work" -v "$MATRIX_OUT:$MATRIX_OUT" -w /work \
    "$IMAGE" "$@"
}

JAR="$HERE/target/matrix.jar"
if [[ ! -f "$JAR" || -n "$(find "$HERE/src" "$HERE/pom.xml" -newer "$JAR" -print -quit)" ]]; then
  echo "[java] building $JAR" >&2
  docker_run mvn -q -B -Dmaven.repo.local=/work/.m2 package
fi

status=0
for sdk in "${SDKS[@]}"; do
  echo "[java] $sdk -> $MATRIX_SERVER ($S3_ENDPOINT)" >&2
  docker_run java -jar target/matrix.jar "$sdk" || status=1
done
exit $status
