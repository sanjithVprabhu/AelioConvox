# Get started — one Docker image + SDKs

The product shape:

1. **Pull & run** a single Aelio image (server + Aelio DB inside)
2. **Install** an SDK (`npm` / `pip` / `go get`)
3. **Expose** tools and `listen()`

## 1. Run (all-in-one)

```bash
docker pull sanjithvprabhu/aelio-server:latest

docker run -d --name aelio \
  -p 3010:3000 \
  -v aelio-data:/data \
  -e AELIO_SDK_SECRET=change-me \
  -e AELIO_LLM_PROVIDER=openai \
  -e OPENAI_API_KEY=sk-... \
  sanjithvprabhu/aelio-server:latest
```

That one container runs:

- **Aelio** on port 3000 (map to 3010)
- **Aelio DB / ll-server** on `127.0.0.1:8080` inside the container (local `.vss` under `/data/aelio-db`)

Health: `curl http://127.0.0.1:3010/health`
Demo: http://127.0.0.1:3010/demo.html
Hub: https://hub.docker.com/r/sanjithvprabhu/aelio-server

### Pick an LLM

| Provider | Env |
|----------|-----|
| **OpenAI** | `-e AELIO_LLM_PROVIDER=openai -e OPENAI_API_KEY=sk-...` |
| **Anthropic** | `-e AELIO_LLM_PROVIDER=anthropic -e ANTHROPIC_API_KEY=sk-ant-...` |
| **Gemini** | `-e AELIO_LLM_PROVIDER=gemini -e GEMINI_API_KEY=...` |
| **Groq** | `-e AELIO_LLM_PROVIDER=groq -e GROQ_API_KEY=...` |

Optional: `-e AELIO_LLM_MODEL=...`

### Local vs cloud `.vss` segments

```bash
# Local (default) — already on with the image
-e AELIO_AELIO DB_SEGMENT_BACKEND=local

# Cloud (AWS S3 / MinIO / R2 / …)
-e AELIO_AELIO DB_SEGMENT_BACKEND=s3 \
-e AELIO_AELIO DB_S3_BUCKET=my-bucket \
-e AELIO_AELIO DB_S3_REGION=us-east-1 \
-e AELIO_AELIO DB_S3_ENDPOINT=https://s3.amazonaws.com \
-e AELIO_AELIO DB_S3_ACCESS_KEY_ID=... \
-e AELIO_AELIO DB_S3_SECRET_ACCESS_KEY=...
```

SQLite-only (no Aelio DB engine): `-e AELIO_AELIO DB_ENABLED=0`

### Env reference

| Variable | Purpose |
|----------|---------|
| `AELIO_SDK_SECRET` | SDK auth (**required**) |
| `AELIO_LLM_PROVIDER` / `AELIO_LLM_MODEL` | Chat provider + model |
| `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` / `GROQ_API_KEY` | Provider key |
| `AELIO_WEB_ALLOWED_ORIGINS` | Widget origins in prod |
| `AELIO_AELIO DB_ENABLED` | `1` (default) or `0` for SQLite-only |
| `AELIO_AELIO DB_SEGMENT_BACKEND` | `local` or `s3` |
| `AELIO_AELIO DB_S3_*` | Cloud credentials / bucket / endpoint |

Details: [aelio-os/docs/segment-storage.md](../aelio-os/docs/segment-storage.md).

## 2. Connect an SDK

Same `AELIO_SDK_SECRET`. Node: `npm i @aelio/sdk` → `aelio.listen({ secret, url: 'ws://127.0.0.1:3010' })`.

## 3. Advanced: split containers

Only if you want to scale Aelio DB separately — image `sanjithvprabhu/aelio-aelio-db` + compose:

```bash
docker compose -f server/docker-compose.yml up -d
```

## Maintainers

```bash
docker login
VERSION=0.1.2 ./scripts/publish-docker.sh
```
