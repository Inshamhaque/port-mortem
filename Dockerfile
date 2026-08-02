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
COPY bench/ bench/
RUN cargo build --release

# Stage 2: prove parity — the original suite, unmodified, against the FFI.
# BuildKit skips stages nothing consumes, so `runtime` below copies a sentinel
# that is only written when `make test-original` passes — making this stage a
# hard build dependency (and therefore a build-time parity gate).
FROM debian:bookworm-slim AS test
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc make libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/libcjson_rs.a /app/target/release/
COPY Makefile ./
COPY tests/ tests/
RUN make test-original && printf 'original suite passed (make test-original)\n' > /app/PARITY_OK

# Stage 3: the runtime image.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/cjson-rs /usr/local/bin/cjson-rs
COPY --from=test /app/PARITY_OK /usr/local/share/cjson-rs/PARITY_OK
ENTRYPOINT ["cjson-rs"]
