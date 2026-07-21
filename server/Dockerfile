# syntax=docker/dockerfile:1
#
# All-in-one Aelio image: Node harness + Astrolobe ll-server in one container.
# Customers: docker pull + docker run — no compose required.

# ---- Astrolobe (ll-server) ----
FROM rust:1-bookworm AS sunjet-build
WORKDIR /src
COPY Sunjet/Astrolobe/ .
RUN cargo build --release -p ll-server

# ---- Aelio Node server ----
FROM node:22-bookworm-slim AS node-build
WORKDIR /app
RUN corepack enable

COPY package.json pnpm-workspace.yaml pnpm-lock.yaml turbo.json tsconfig.base.json ./
COPY server ./server
COPY chat ./chat
COPY packages ./packages
COPY config.yaml ./config.yaml
COPY config.docker.yaml ./config.docker.yaml

RUN pnpm install --frozen-lockfile
RUN pnpm build
RUN pnpm --filter @aelio/server deploy --prod /app/bundle

# ---- Runtime: both processes via entrypoint ----
FROM node:22-bookworm-slim AS runtime

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl tini \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app

ENV NODE_ENV=production
ENV AELIO_CONFIG=/app/config.yaml
ENV AELIO_PUBLIC_PATH=/app/public
ARG AELIO_VERSION=0.1.2
ENV AELIO_VERSION=$AELIO_VERSION

# All-in-one defaults (override with -e …)
ENV LL_DATA_DIR=/data/sunjet
ENV LL_BIND=127.0.0.1:8080
ENV AELIO_SUNJET_ENABLED=1
ENV AELIO_SUNJET_URL=http://127.0.0.1:8080
ENV AELIO_SUNJET_SEGMENT_BACKEND=local

COPY --from=sunjet-build /src/target/release/ll-server /usr/local/bin/ll-server
COPY --from=node-build /app/bundle/node_modules ./node_modules
COPY --from=node-build /app/bundle/dist ./dist
COPY --from=node-build /app/bundle/public ./public
COPY --from=node-build /app/config.docker.yaml ./config.yaml
COPY server/docker-entrypoint.sh /usr/local/bin/aelio-entrypoint.sh
RUN chmod +x /usr/local/bin/aelio-entrypoint.sh /usr/local/bin/ll-server

VOLUME ["/data"]
EXPOSE 3000 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=25s --retries=3 \
  CMD ["curl", "-fsS", "http://127.0.0.1:3000/health"]

ENTRYPOINT ["tini", "--", "/usr/local/bin/aelio-entrypoint.sh"]
