FROM docker.io/library/rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc AS server-builder

ARG SOURCE_REV
LABEL org.opencontainers.image.revision="${SOURCE_REV}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates \
        clang \
        cmake \
        curl \
        git \
        libssl-dev \
        pkg-config \
        python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . /src
RUN test -f /src/Cargo.lock && test -f /src/crates/tm-server/Cargo.toml

ENV CARGO_BUILD_JOBS=2
RUN cargo build --locked --release --package tm-server

FROM ghcr.io/cirruslabs/flutter:3.41.9@sha256:c6fed8fee02ca8fdb1b8dc128df99f6e29b7305847b80f4b0c1e3f638e7637a8 AS web-builder

WORKDIR /src
COPY . /src
RUN test -f /src/clients/miku_flutter/pubspec.lock

WORKDIR /src/clients/miku_flutter
RUN flutter pub get \
    && flutter build web --release --no-pub

FROM docker.io/library/debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates \
        libssl3 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=server-builder /src/target/release/tm-server /usr/local/bin/tm-server
COPY --from=web-builder /src/clients/miku_flutter/build/web /usr/local/share/tempestmiku/web

ENV TM_WEBUI_DIR=/usr/local/share/tempestmiku/web

ENTRYPOINT ["/usr/local/bin/tm-server"]
