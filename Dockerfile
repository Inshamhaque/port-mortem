# syntax=docker/dockerfile:1
#
# One command to a runnable artifact — and a build-time proof of parity: the
# `test` stage compiles and runs the unmodified original C test suite against
# the Rust FFI layer, so `docker build` fails if the port ever regresses.
#
#   docker build -t cjson-rs .
#   echo '{"hello":"world"}' | docker run --rm -i cjson-rs parse -

# Stage 1: build the crate (rlib + staticlib + CLI).
FROM rust:1.97-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

# Stage 2: prove parity — the original suite, unmodified, against the FFI.
FROM debian:bookworm-slim AS test
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc make \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/libcjson_rs.a /app/target/release/
COPY Makefile ./
COPY tests/ tests/
RUN make test-original

# Stage 3: the runtime image.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cjson-rs /usr/local/bin/cjson-rs
ENTRYPOINT ["cjson-rs"]
