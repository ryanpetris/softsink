FROM docker.io/library/archlinux:base-devel

RUN pacman -Syu --noconfirm --needed python \
    && pacman -Scc --noconfirm \
    && useradd -m builder
