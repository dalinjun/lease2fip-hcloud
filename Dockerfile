# Configure the rust toolchain and cache it.
FROM rust:1 AS rust-toolchain
WORKDIR /build
COPY rust-toolchain.toml .
COPY scripts scripts
RUN scripts/configure-rust && cargo install cargo-chef

# Prepare cargo chef plan.
FROM rust-toolchain AS planner
WORKDIR /build
COPY Cargo.toml Cargo.lock .
RUN cargo chef prepare --recipe-path recipe.json

# Prepare builder image.
FROM rust-toolchain AS builder
WORKDIR /build

# Build dependencies and cache them.
COPY --from=planner /build/recipe.json recipe.json
RUN RUSTFLAGS="-D warnings" cargo chef cook --clippy --locked --recipe-path recipe.json --release && RUSTFLAGS="-D warnings" cargo chef cook --locked --recipe-path recipe.json --release

# Copy the source code and build the binary.
COPY Cargo.toml Cargo.lock .rustfmt.toml build.rs .
COPY src src
ARG GIT_COMMIT_HASH
ENV GIT_COMMIT_HASH=${GIT_COMMIT_HASH}
RUN scripts/check-formatting && scripts/lint && scripts/build-binary

# Prepare runner image.
FROM gcr.io/distroless/cc-debian12 AS runner
COPY --from=builder /build/target/release/lease2fip-hcloud /lease2fip-hcloud
ENTRYPOINT ["/lease2fip-hcloud"]
