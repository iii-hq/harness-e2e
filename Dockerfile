# The executor: the tools an execution's phases run with, and nothing else.
# The scripts are mounted per execution by scripts/run_in_image.sh, which also
# names this image: ghcr.io/iii-hq/harness-e2e:tools-<first 12 hex of this
# file's sha256>. Any change here is a new tag, published from main by
# .github/workflows/executor-image.yml; until then the wrapper builds it.
# Everything it installs is pinned, so the same Dockerfile is the same tools.
FROM docker:29.1.3-cli@sha256:4fa0ee1f3a7e4354c4ea34558b6d4ee32859baf4973d4c8ccc8e7fe3dd730c04 AS docker-cli
FROM docker:29.1.3-dind@sha256:173f284a4299164772a90f52b373e73e087583c0963f1334c9995f190ef6f3f5 AS docker-engine

# The Actions runner's distribution: the workers the stacks install are built
# for its glibc. This build (2026-09-11) predates the apt snapshot below.
FROM ubuntu:24.04@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3
LABEL org.opencontainers.image.source=https://github.com/iii-hq/harness-e2e
ARG DEBIAN_FRONTEND=noninteractive

# Every apt call reads one snapshot of the Ubuntu archive
# (snapshot.ubuntu.com): the same packages for the same Dockerfile, never an
# index from a mirror node that lists what another node's pool lacks. The
# snapshot is fetched over https, so the first call trusts the CA bundle of
# the Docker CLI image until ca-certificates is installed.
COPY --from=docker-cli /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN printf 'APT::Snapshot "20260924T000000Z";\nAcquire::Retries "3";\n' >/etc/apt/apt.conf.d/50snapshot \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl git iptables jq procps python3 python3-pip python3-yaml unzip xz-utils \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSLo /tmp/node.tar.xz https://nodejs.org/dist/v24.18.0/node-v24.18.0-linux-x64.tar.xz \
 && echo "55aa7153f9d88f28d765fcdad5ae6945b5c0f98a36881703817e4c450fa76742  /tmp/node.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --no-same-owner \
 && rm /tmp/node.tar.xz \
 && npm install -g pnpm@11.13.1 \
 && rm -rf /root/.npm

# The browser worker drives a system Chromium (/usr/bin/chromium); Playwright
# brings one and the libraries it needs. A container has no user namespaces
# for Chromium's sandbox (it aborts: "No usable sandbox!") and a 64 MB
# /dev/shm, so /usr/bin/chromium starts it without either; the container is
# the sandbox.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npx -y playwright@1.62.1 install --with-deps --no-shell chromium \
 && printf '#!/bin/sh\nexec %s --no-sandbox --disable-dev-shm-usage "$@"\n' \
      "$(find /opt/ms-playwright -type f -path '*/chrome-linux*/chrome' | head -n1)" >/usr/bin/chromium \
 && chmod 755 /usr/bin/chromium \
 && chromium --version \
 && rm -rf /root/.npm /var/lib/apt/lists/*

RUN curl -fsSLo /tmp/go.tar.gz https://go.dev/dl/go1.25.14.linux-amd64.tar.gz \
 && echo "a21ae5633a269bcd7e90cf767e48225633795e99d831742cbf3397064fee7712  /tmp/go.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/go.tar.gz -C /usr/local \
 && rm /tmp/go.tar.gz

# Writable by any user, as the official rust image leaves them.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
RUN curl -fsSLo /tmp/rustup-init https://static.rust-lang.org/rustup/archive/1.29.1/x86_64-unknown-linux-gnu/rustup-init \
 && echo "dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71  /tmp/rustup-init" | sha256sum -c - \
 && chmod +x /tmp/rustup-init \
 && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain 1.98.1 \
 && rm /tmp/rustup-init \
 && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# What the ubuntu-latest runner the groups used to run on gives a subject as
# well: gh, and pip installing for the user as it does there.
RUN curl -fsSLo /tmp/gh.tar.gz https://github.com/cli/cli/releases/download/v2.101.0/gh_2.101.0_linux_amd64.tar.gz \
 && echo "9bca2d1c16825f109907a23307628a2f0698fbf99662b73a5cf0b020293072b8  /tmp/gh.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/gh.tar.gz -C /usr/local --strip-components=1 gh_2.101.0_linux_amd64/bin/gh \
 && rm /tmp/gh.tar.gz
ENV PIP_BREAK_SYSTEM_PACKAGES=1

# A group runs a Docker daemon of its own (scripts/executor.sh starts it), in
# which Registry, trending_topics_build and Kanban start their fixtures: the
# engine's static binaries, of the CLI's release, and iptables above for its
# networks. compose is the runner's plugin.
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /usr/local/libexec/docker/cli-plugins/docker-buildx /usr/local/libexec/docker/cli-plugins/docker-compose \
     /usr/local/libexec/docker/cli-plugins/
COPY --from=docker-engine /usr/local/bin/dockerd /usr/local/bin/containerd /usr/local/bin/containerd-shim-runc-v2 \
     /usr/local/bin/runc /usr/local/bin/docker-init /usr/local/bin/docker-proxy /usr/local/bin/

ENV PATH=/home/executor/.local/bin:/usr/local/cargo/bin:/usr/local/go/bin:$PATH
# Never root (Kanban refuses it), and any --user works: HOME is writable by
# every uid, and scripts/executor.sh names a uid /etc/passwd does not know
# (run_in_image.sh adds no-new-privileges, so that entry cannot reach root).
# A group alone starts as root, for its Docker daemon, and runs as the user.
ENV HOME=/home/executor
# iii's anonymous product-usage telemetry stays off in every phase and in
# every engine and worker a group starts.
ENV III_TELEMETRY_ENABLED=false
RUN install -d -m 1777 /home/executor && chmod a+w /etc/passwd
USER ubuntu
WORKDIR /home/executor
