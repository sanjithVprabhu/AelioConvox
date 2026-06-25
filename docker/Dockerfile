# syntax=docker/dockerfile:1

# ---- build stage: install everything, build the workspace, prune to prod ----
FROM node:22-bookworm-slim AS build

WORKDIR /app
RUN corepack enable

COPY package.json pnpm-workspace.yaml pnpm-lock.yaml turbo.json tsconfig.base.json ./
COPY apps ./apps
COPY packages ./packages
COPY config.yaml ./config.yaml
COPY config.docker.yaml ./config.docker.yaml

RUN pnpm install --frozen-lockfile
RUN pnpm build

# Produce a self-contained, production-only deployment of the server.
# pnpm deploy flattens the workspace deps (@aelio/*) and their prod deps
# (incl. the compiled better-sqlite3 / sqlite-vec native bindings) into one tree.
RUN pnpm --filter @aelio/server deploy --prod /app/bundle

# ---- runtime stage: distroless, node-only, no shell ----
FROM gcr.io/distroless/nodejs22-debian12 AS runtime

WORKDIR /app

ENV NODE_ENV=production
ENV AELIO_CONFIG=/app/config.yaml
ENV AELIO_MIGRATIONS_PATH=/app/node_modules/@aelio/db/drizzle
ENV AELIO_PUBLIC_PATH=/app/public

COPY --from=build /app/bundle/node_modules ./node_modules
COPY --from=build /app/bundle/dist ./dist
COPY --from=build /app/bundle/public ./public
COPY --from=build /app/config.docker.yaml ./config.yaml

VOLUME ["/data"]
EXPOSE 3000

# distroless has no shell, so the healthcheck must be exec-form with the node path.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/nodejs/bin/node", "-e", "fetch('http://127.0.0.1:3000/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"]

# The distroless nodejs image's entrypoint is already node, so CMD is just the script.
CMD ["dist/main.js"]
