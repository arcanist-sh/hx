FROM debian:bookworm-slim

ARG TARGETARCH
ARG VERSION=0.9.1

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN case "${TARGETARCH}" in \
        amd64) ARCH="x86_64-unknown-linux-gnu" ;; \
        arm64) ARCH="aarch64-unknown-linux-gnu" ;; \
        *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/arcanist-sh/hx/releases/download/v${VERSION}/hx-v${VERSION}-${ARCH}.tar.gz" | tar xz -C /usr/local/bin

RUN hx --version

ENTRYPOINT ["hx"]
CMD ["--help"]
