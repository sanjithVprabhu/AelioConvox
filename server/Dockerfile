# syntax=docker/dockerfile:1
# One Aelio image: authoritative Rust OS/database plus a thin TypeScript edge.

FROM rust:1-bookworm AS rust-build
WORKDIR /src
COPY aelio-os/ .
RUN cargo build --release -p aelio-server

FROM node:22-bookworm-slim AS node-build
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-workspace.yaml pnpm-lock.yaml turbo.json tsconfig.base.json ./
COPY server ./server
COPY chat ./chat
COPY packages ./packages
COPY config.yaml config.docker.yaml ./
RUN pnpm install --frozen-lockfile
RUN pnpm build
RUN pnpm --filter @aelio/server deploy --prod /app/bundle

FROM node:22-bookworm-slim AS runtime
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl tini \
  && rm -rf /var/lib/apt/lists/*
WORKDIR /app

ENV NODE_ENV=production
ENV AELIO_CONFIG=/app/config.yaml
ENV AELIO_PUBLIC_PATH=/app/public
ENV AELIO_DATA_DIR=/data
ENV AELIO_RUNTIME_BIND=127.0.0.1:8090
ENV AELIO_RUST_RUNTIME_URL=http://127.0.0.1:8090
ENV AELIO_HOST_URL=http://127.0.0.1:3000
ENV AELIO_DB_ENABLED=1
ENV AELIO_DB_URL=http://127.0.0.1:8090

COPY --from=rust-build /src/target/release/aelio-server /usr/local/bin/aelio-server
COPY --from=node-build /app/bundle/node_modules ./node_modules
COPY --from=node-build /app/bundle/dist ./dist
COPY --from=node-build /app/bundle/public ./public
COPY --from=node-build /app/config.docker.yaml ./config.yaml
COPY server/docker-entrypoint.sh /usr/local/bin/aelio-entrypoint.sh
RUN chmod +x /usr/local/bin/aelio-entrypoint.sh /usr/local/bin/aelio-server

VOLUME ["/data"]
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=25s --retries=3 \
  CMD ["curl", "-fsS", "http://127.0.0.1:3000/ready"]
ENTRYPOINT ["tini", "--", "/usr/local/bin/aelio-entrypoint.sh"]
