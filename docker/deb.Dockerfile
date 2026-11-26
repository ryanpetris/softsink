ARG BASE=docker.io/library/debian:13
FROM ${BASE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential debhelper libasound2t64 libpipewire-0.3-0t64 \
        libwayland-client0 libxkbcommon0 python3 \
    && rm -rf /var/lib/apt/lists/*
