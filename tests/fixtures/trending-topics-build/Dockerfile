FROM node:24.18.0-bookworm-slim@sha256:d45d78e7929b46875bbd4e29bea672d5bc48186c6c3588306521c815e78352d6 AS node
FROM mcr.microsoft.com/playwright:v1.62.1-noble@sha256:c091b21d9fae78c76e85cd4356431e9b018402f172a214fc7d7a5e9a7e29d8ac
ENV III_TELEMETRY_ENABLED=false

COPY --from=node /usr/local/ /usr/local/
RUN apt-get update && apt-get install -y --no-install-recommends git coreutils python3 \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global npm@11.6.2 --ignore-scripts --no-audit --no-fund
COPY package.json package-lock.json /opt/public-deps/
RUN cd /opt/public-deps && npm ci --cache /opt/npm-cache --no-audit --no-fund \
    && chmod -R a+rX /opt/public-deps /opt/npm-cache \
    && test "$(node --version)" = v24.18.0 && test "$(npm --version)" = 11.6.2

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    NPM_CONFIG_CACHE=/tmp/npm-cache \
    NPM_CONFIG_OFFLINE=true \
    NPM_CONFIG_AUDIT=false \
    NPM_CONFIG_FUND=false \
    GIT_CONFIG_NOSYSTEM=1 \
    GIT_CONFIG_GLOBAL=/dev/null \
    GIT_AUTHOR_NAME="Harness delivery" \
    GIT_AUTHOR_EMAIL=delivery@example.invalid \
    GIT_COMMITTER_NAME="Harness delivery" \
    GIT_COMMITTER_EMAIL=delivery@example.invalid
WORKDIR /workspace
USER 1000:1000
