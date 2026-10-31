#!/usr/bin/env bash
# Runs the .NET SDK matrix (AWSSDK.S3 + Minio) inside the official .NET SDK image.
# NuGet packages are cached in ./.nuget, build output in ./bin and ./obj.
set -euo pipefail
DIR=$(cd "$(dirname "$0")" && pwd)
: "${S3_ENDPOINT:?S3_ENDPOINT required}"
: "${MATRIX_SERVER:?MATRIX_SERVER required}"
OUT=${MATRIX_OUT:-$DIR/../results/$MATRIX_SERVER}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
IMAGE=${DOTNET_IMAGE:-mcr.microsoft.com/dotnet/sdk:10.0}

docker run --rm --network host --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e DOTNET_CLI_HOME=/work/.dotnet-home -e NUGET_PACKAGES=/work/.nuget/packages \
  -e DOTNET_NOLOGO=1 -e DOTNET_CLI_TELEMETRY_OPTOUT=1 -e DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1 \
  -e S3_ENDPOINT -e S3_ACCESS_KEY="${S3_ACCESS_KEY:-fakes3}" -e S3_SECRET_KEY="${S3_SECRET_KEY:-fakes3secret}" \
  -e S3_REGION="${S3_REGION:-us-east-1}" -e MATRIX_SERVER -e MATRIX_OUT=/out \
  -v "$DIR:/work" -v "$OUT:/out" -w /work "$IMAGE" \
  sh -c 'dotnet build -c Release --nologo -v q -clp:ErrorsOnly >/dev/null || dotnet build -c Release --nologo -v q -clp:ErrorsOnly; exec dotnet bin/Release/net10.0/Matrix.dll'
