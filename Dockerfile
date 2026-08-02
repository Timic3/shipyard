# ── Stage 1: build ────────────────────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder

WORKDIR /build

# Pre-fetch and compile dependencies before copying source so that Docker layer
# caching re-uses this step when only src/ changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

# Now build the real binary.
COPY src ./src
# Touch main.rs so cargo knows it changed (the dep step compiled a stub).
RUN touch src/main.rs \
    && cargo build --release \
    && strip target/release/shipyard

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
# distroless/cc gives us glibc + libgcc + CA certificates and nothing else.
# There is no shell, no package manager, and no writable filesystem by default.
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/target/release/shipyard /shipyard

# config.toml is expected to be mounted via a ConfigMap/Secret volume at
# /config/config.toml and passed as the first argument (see CMD below).
# GITHUB_TOKEN must be injected as an environment variable (Secret).

EXPOSE 8080

ENTRYPOINT ["/shipyard"]
CMD ["/config/config.toml"]
