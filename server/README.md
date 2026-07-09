# Aelio Server

The `@aelio/server` runtime — the agentic harness your Convox SDK and chat widget both connect to. Fastify + `@fastify/websocket`, wrapping the runtime in [`packages/core`](../packages/core): the LLM tool loop, safety rails, identity, memory/recall, and the **Sunjet/Astrolobe** Rust storage engine (with SQLite + `sqlite-vec` fallback).

This is meant to ship as a **Docker image** you pull and run; the SDKs point at it.

## Endpoints

| Path | Protocol | Purpose |
|---|---|---|
| `/health`, `/ready` | HTTP | Liveness / readiness (SDK connected, vector index, etc.) |
| `/sdk` | WebSocket | Convox SDK connects here (`Authorization: Bearer <secret>`) |
| `/widget/ws` | WebSocket | Browser chat widget connects here (origin-gated) |
| `/widget.js` | HTTP | Serves the built `@aelio/chat` IIFE bundle |
| `/wa/webhook` | HTTP | WhatsApp (Meta) inbound webhook |
| `/auth/*`, `/proactive/*`, `/telemetry` | HTTP | Magic-link auth, proactive outreach, telemetry |

## Run locally

```bash
export AELIO_SDK_SECRET=change-me-in-production
pnpm --filter @aelio/server dev      # tsx watch, reads ../config.yaml
```

Config is `config.yaml` at the repo root (schema in [`src/config.ts`](src/config.ts), `${ENV}` interpolation). Key knobs: `llm`, `embeddings`, `channels.web.allowed_origins`, `channels.whatsapp`, `safety`, `sunjet`, `storage`.

## Docker

```bash
pnpm docker:build                    # docker build -f server/Dockerfile -t aelio/server:latest .
pnpm docker:up                       # docker compose -f server/docker-compose.yml up --build
pnpm docker:verify                   # build, run container, run all phase tests
```

The [Dockerfile](Dockerfile) is a two-stage build (build context is the **repo root**): install the workspace, `pnpm build` (which compiles `@aelio/chat` first so `widget.js` lands in `public/`), then `pnpm --filter @aelio/server deploy --prod` into a distroless runtime. Inside the container the config is `config.docker.yaml`.

Environment: `AELIO_CONFIG`, `AELIO_SDK_SECRET`, `AELIO_MIGRATIONS_PATH`, `AELIO_PUBLIC_PATH`, plus whatever your config references via `${...}`. Data volume is `/data`.

## Deploy templates

One-click templates live alongside this README: [`railway.toml`](railway.toml), [`render.yaml`](render.yaml), [`fly.toml`](fly.toml). Each points `dockerfile` at `server/Dockerfile` with the repo root as build context.

See [docs/MANUAL.md](../docs/MANUAL.md) for the full configuration and deployment reference.
