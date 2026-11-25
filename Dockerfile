# AI-Native OS Orchestrator - Docker Image
# Multi-stage build for minimal production image

# Build stage
FROM rust:1.78-bookworm AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    llvm \
    libclang-dev \
    libbpf-dev \
    linux-headers-generic \
    pkg-config \
    libssl-dev \
    libgit2-dev \
    protobuf-compiler \
    cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace files
COPY Cargo.toml ./
COPY crates/common ./crates/common
COPY crates/orchestrator-core ./crates/orchestrator-core
COPY crates/orchestrator-observe ./crates/orchestrator-observe
COPY crates/orchestrator-plan ./crates/orchestrator-plan
COPY crates/orchestrator-act ./crates/orchestrator-act
COPY crates/orchestrator-validate ./crates/orchestrator-validate
COPY crates/orchestrator-federate ./crates/orchestrator-federate
COPY crates/orchestrator-ui ./crates/orchestrator-ui
COPY crates/orchestrator-policy ./crates/orchestrator-policy

# Build release
RUN cargo build --release --workspace

# Runtime stage
FROM debian:12-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    libgit2-1.7 \
    libbpf1 \
    libelf1 \
    libzstd1 \
    sqlite3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create orchestrator user
RUN groupadd -r orchestrator && useradd -r -g orchestrator -d /var/lib/orchestrator -s /bin/bash orchestrator

# Copy binary
COPY --from=builder /app/target/release/orchestrator /usr/local/bin/orchestrator

# Copy config example
COPY --from=builder /app/config.example.toml /etc/orchestrator/config.example.toml

# Create directories
RUN mkdir -p /var/lib/orchestrator/{models,lora,rag,federation} \
    /etc/orchestrator/policies \
    /var/log/orchestrator \
    && chown -R orchestrator:orchestrator /var/lib/orchestrator /etc/orchestrator /var/log/orchestrator

# Switch to non-root user
USER orchestrator

# Environment
ENV RUST_LOG=info \
    ORCHESTRATOR_CONFIG=/etc/orchestrator/config.toml \
    ORCHESTRATOR_MODEL_PATH=/var/lib/orchestrator/models/qwen2.5-coder-32b-4bit.gguf \
    ORCHESTRATOR_LORA_PATH=/var/lib/orchestrator/lora \
    ORCHESTRATOR_RAG_PATH=/var/lib/orchestrator/rag

# Expose web UI port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Entrypoint
ENTRYPOINT ["orchestrator"]
CMD ["--config", "/etc/orchestrator/config.toml"]