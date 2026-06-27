FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      attr \
      bash \
      build-essential \
      ca-certificates \
      coreutils \
      curl \
      fuse-overlayfs \
      fuse3 \
      git \
      jq \
      libcap2-bin \
      libfuse3-dev \
      pkg-config \
      sudo \
      util-linux \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"
WORKDIR /work/kage
