# runx CLI container image.
#
# The binary is pulled from the GitHub Release hub (the same musl archive every
# other channel consumes), so the image build needs no Rust toolchain and stays
# multi-arch without QEMU: the fetch stage runs on the build platform and selects
# the binary matching the requested TARGETARCH.

ARG VERSION
ARG SHA256_AMD64
ARG SHA256_ARM64
ARG CLI_COMMIT
ARG PACKAGING_COMMIT

FROM --platform=$BUILDPLATFORM alpine:3@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS fetch
ARG VERSION
ARG SHA256_AMD64
ARG SHA256_ARM64
ARG TARGETARCH
RUN apk add --no-cache curl tar
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl; expected_sha256="$SHA256_AMD64" ;; \
      arm64) target=aarch64-unknown-linux-musl; expected_sha256="$SHA256_ARM64" ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    test -n "$expected_sha256"; \
    base="https://github.com/runxhq/runx/releases/download/cli-v${VERSION}"; \
    curl -fsSL "${base}/runx-${VERSION}-${target}.tar.gz" -o /tmp/runx.tgz; \
    echo "$expected_sha256  /tmp/runx.tgz" | sha256sum -c -; \
    tar -xzf /tmp/runx.tgz -C /tmp; \
    install -Dm755 "/tmp/runx-${VERSION}-${target}/runx" /out/runx; \
    install -Dm755 "/tmp/runx-${VERSION}-${target}/runx-js-worker" /out/runx-js-worker

FROM gcr.io/distroless/static-debian12:nonroot@sha256:afa5c872c891853ca7fcf1f12c3edb23f7eeef36189728842dd51042ff57f7ab
ARG VERSION
ARG CLI_COMMIT
ARG PACKAGING_COMMIT
LABEL org.opencontainers.image.source="https://github.com/runxhq/runx" \
      org.opencontainers.image.version="$VERSION" \
      org.opencontainers.image.revision="$PACKAGING_COMMIT" \
      io.runx.cli.revision="$CLI_COMMIT"
COPY --from=fetch /out/runx /usr/local/bin/runx
COPY --from=fetch /out/runx-js-worker /usr/local/bin/runx-js-worker
ENTRYPOINT ["/usr/local/bin/runx"]
