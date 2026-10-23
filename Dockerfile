# syntax=docker/dockerfile:1
# Kage server + facilitator image (one image, two binaries). Multi-stage: build on the pinned
# toolchain, run on debian slim as an unprivileged user. No secrets are baked in: every key is a
# runtime env var (see docker-compose.yml and .env.example).
FROM rust:1.89-bookworm AS build
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} CARGO_TERM_COLOR=never
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p x402z-server -p x402z-facilitator \
 && mkdir -p /out && cp target/release/x402z-server target/release/x402z-facilitator /out/

FROM debian:bookworm-slim AS run
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data kage && mkdir -p /data && chown kage:kage /data && chmod 700 /data
COPY --from=build /out/x402z-server /out/x402z-facilitator /usr/local/bin/
USER kage
WORKDIR /data
CMD ["x402z-server"]
