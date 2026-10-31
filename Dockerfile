# syntax=docker/dockerfile:1
#
# Static musl build on Alpine; the runtime image is Alpine plus one binary.
#
#   docker build -t fakes3 .
#   docker run --rm -p 9000:9000 fakes3
#
# Signatures are not verified by default (any credentials work); `-e FAKES3_AUTH=1` verifies them
# against FAKES3_ACCESS_KEY / FAKES3_SECRET_KEY.

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev perl make linux-headers
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
 && install -s target/release/fakes3 /fakes3

# `docker build --target binary --output out .` exports just the static binary (used for releases).
FROM scratch AS binary
COPY --from=build /fakes3 /fakes3

FROM alpine:3
COPY --from=build /fakes3 /usr/local/bin/fakes3
ENV FAKES3_HOST=0.0.0.0 FAKES3_PORT=9000
EXPOSE 9000
ENTRYPOINT ["fakes3"]
