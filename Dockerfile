# syntax=docker/dockerfile:1.26.0@sha256:ecfaec9ed6d810b56388c508f4121597bfbba70d41a6dfeee4d8cad5f295fc32

FROM rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc AS builder

ARG VERSION
ARG VCS_REF
ARG SOURCE_DATE_EPOCH
ARG BUILD_DATE

WORKDIR /source
COPY . .

RUN test -n "${VERSION}" \
    && test -n "${VCS_REF}" \
    && test -n "${SOURCE_DATE_EPOCH}" \
    && test -n "${BUILD_DATE}" \
    && test "$(cat VERSION)" = "${VERSION}" \
    && test "$(sed -n 's/^version = "\([^"]*\)"/\1/p' prnsd/Cargo.toml | head -n 1)" = "${VERSION}" \
    && test "${#VCS_REF}" -eq 40 \
    && case "${VCS_REF}" in *[!0-9a-f]*) exit 1 ;; esac \
    && case "${SOURCE_DATE_EPOCH}" in *[!0-9]*) exit 1 ;; esac

ENV CARGO_PROFILE_RELEASE_STRIP=symbols
ENV CARGO_TARGET_DIR=/tmp/prnsd-target
ENV RUSTUP_TOOLCHAIN=1.96.0
ENV SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}

RUN test "$(rustc --version | cut -d ' ' -f 2)" = "1.96.0" \
    && PRNS_BUILD_COMMIT="${VCS_REF}" cargo build \
    --manifest-path prnsd/Cargo.toml \
    --locked \
    --release \
    --no-default-features \
    --features tokio-cloud-host,observability,otlp \
    && install -D -m 0755 /tmp/prnsd-target/release/prnsd /image/usr/local/bin/prnsd \
    && install -D -m 0644 LICENSE-APACHE /image/usr/share/doc/prnsd/LICENSE-APACHE \
    && install -D -m 0644 LICENSE-MIT /image/usr/share/doc/prnsd/LICENSE-MIT \
    && install -D -m 0644 THIRD_PARTY_NOTICES.md /image/usr/share/doc/prnsd/THIRD_PARTY_NOTICES.md \
    && install -D -m 0644 release/keys/minisign.pub /image/usr/share/doc/prnsd/minisign.pub \
    && if test -f release/source-bundle/source.zip; then \
    install -D -m 0644 release/source-bundle/source.zip /image/usr/share/prnsd/source.zip; \
    install -D -m 0644 release/source-bundle/source.zip.sha256 /image/usr/share/prnsd/source.zip.sha256; \
    fi \
    && install -d -m 0700 -o 65532 -g 65532 /image/var/lib/prnsd \
    && rm -rf /tmp/prnsd-target /usr/local/cargo/registry

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

ARG VERSION
ARG VCS_REF
ARG SOURCE_DATE_EPOCH
ARG BUILD_DATE

LABEL org.opencontainers.image.title="prnsd" \
    org.opencontainers.image.description="Containerized headless Prns daemon" \
    org.opencontainers.image.source="https://github.com/KenAKAFrosty/Prns" \
    org.opencontainers.image.url="https://reticulum.rs" \
    org.opencontainers.image.documentation="https://github.com/KenAKAFrosty/Prns/tree/main/docs" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.version="${VERSION}" \
    org.opencontainers.image.revision="${VCS_REF}" \
    org.opencontainers.image.created="${BUILD_DATE}"

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /image /

USER 65532:65532
ENV PRNSD_STATE_DIR=/var/lib/prnsd/.service
VOLUME ["/var/lib/prnsd"]
EXPOSE 4242/tcp 4284/tcp
STOPSIGNAL SIGTERM

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["/usr/local/bin/prnsd", "status", "--json"]

ENTRYPOINT ["/usr/local/bin/prnsd"]
CMD ["run", "--service", "--config", "/var/lib/prnsd", "--persistence-policy", "required", "--bootstrap", "server", "--log-format", "json"]
