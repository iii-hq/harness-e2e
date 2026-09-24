# The executor: the tools an execution's phases run with, and nothing else.
# The scripts are mounted per execution by scripts/run_in_image.sh, which also
# names this image: ghcr.io/iii-hq/harness-e2e:tools-<first 12 hex of this
# file's sha256>. Any change here is a new tag, published from main by
# .github/workflows/executor-image.yml; until then the wrapper builds it.
FROM docker:29.1.3-cli AS docker-cli

# The Actions runner's distribution: the workers the stacks install are built
# for its glibc.
FROM ubuntu:24.04
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
      build-essential ca-certificates curl git jq procps python3 python3-yaml unzip xz-utils \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSLo /tmp/node.tar.xz https://nodejs.org/dist/v24.18.0/node-v24.18.0-linux-x64.tar.xz \
 && echo "55aa7153f9d88f28d765fcdad5ae6945b5c0f98a36881703817e4c450fa76742  /tmp/node.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --no-same-owner \
 && rm /tmp/node.tar.xz \
 && npm install -g pnpm@11.13.1 \
 && rm -rf /root/.npm

# The browser worker drives a system Chromium (/usr/bin/chromium); Playwright
# brings one and the libraries it needs.
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
RUN npx -y playwright@1.62.1 install --with-deps --no-shell chromium \
 && ln -s "$(find /opt/ms-playwright -type f -path '*/chrome-linux*/chrome' | head -n1)" /usr/bin/chromium \
 && chromium --version \
 && rm -rf /root/.npm /var/lib/apt/lists/*

RUN curl -fsSLo /tmp/go.tar.gz https://go.dev/dl/go1.25.14.linux-amd64.tar.gz \
 && echo "a21ae5633a269bcd7e90cf767e48225633795e99d831742cbf3397064fee7712  /tmp/go.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/go.tar.gz -C /usr/local \
 && rm /tmp/go.tar.gz

# Writable by any user, as the official rust image leaves them.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
 && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# trending_topics_build and Kanban start their fixtures through the host's
# Docker socket.
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=docker-cli /usr/local/libexec/docker/cli-plugins/docker-buildx /usr/local/libexec/docker/cli-plugins/docker-buildx

ENV PATH=/usr/local/cargo/bin:/usr/local/go/bin:$PATH
# Never root (Kanban refuses it), and any --user works: HOME is writable by
# every uid, and scripts/executor.sh names a uid /etc/passwd does not know.
ENV HOME=/home/executor
RUN install -d -m 1777 /home/executor && chmod a+w /etc/passwd
USER ubuntu
WORKDIR /home/executor
