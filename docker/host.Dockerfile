ARG BASE=docker.io/library/ubuntu:26.04
FROM ${BASE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential ca-certificates cmake curl libasound2-dev libclang-dev \
        libpipewire-0.3-dev linux-libc-dev nasm pkg-config python3 \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_TOOLCHAIN=1.99.1 RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --no-modify-path --profile minimal --default-toolchain "$RUSTUP_TOOLCHAIN" \
    && rm /tmp/rustup.sh
