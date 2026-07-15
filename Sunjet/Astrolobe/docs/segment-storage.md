# Astrolobe segment storage (local vs cloud)

Immutable `.vss` segment files (vectors, BM25 text indexes, graph edges) can live on
**local disk** or an **S3-compatible object store**. Hot metadata always stays local.

## What stays local (always)

| File | Why |
|------|-----|
| `wal.log` | Append + fsync — not object-store friendly |
| `manifest.bin` | Atomic commit of the segment set |
| `catalog.bin` | Schema needed before any segment fetch |

## What goes to cloud when `backend=s3`

| File | Lifecycle |
|------|-----------|
| `seg-NNNNN.vss` | **Publish** on flush (before manifest swing) |
| Old `seg-*.vss` | **Pruned** from local + cloud after compact |

Search path: open → `ensure_local` downloads missing segments into `LL_DATA_DIR` →
query in memory (same as today). This is a **write-through local cache**.

## Env (Aelio deployment — secrets never in YAML)

```bash
# Choose backend
export AELIO_SUNJET_SEGMENT_BACKEND=s3   # or local

# S3-compatible (AWS S3, MinIO, Cloudflare R2, Backblaze B2, …)
export AELIO_SUNJET_S3_BUCKET=my-bucket
export AELIO_SUNJET_S3_REGION=us-east-1
export AELIO_SUNJET_S3_ENDPOINT=https://s3.amazonaws.com   # or http://127.0.0.1:9000 for MinIO
export AELIO_SUNJET_S3_ACCESS_KEY_ID=...
export AELIO_SUNJET_S3_SECRET_ACCESS_KEY=...
export AELIO_SUNJET_SEGMENT_PREFIX=prod/aelio/             # optional

# MinIO extras
export AELIO_SUNJET_S3_ALLOW_HTTP=1
export AELIO_SUNJET_S3_PATH_STYLE=1
```

`ll-server` also accepts `LL_*` aliases (`LL_SEGMENT_BACKEND`, `LL_S3_BUCKET`, …).
**Forward the same env into the ll-server process** (docker-compose `env_file`, systemd, etc.).

## Start

```bash
# ll-server picks up AELIO_SUNJET_* / LL_* automatically
LL_DATA_DIR=./data/sunjet cargo run --release -p ll-server

# Health reports the backend:
# { "status":"ok", "segment_backend":"local" | "s3-cached", ... }
```

## Crash safety

1. Write+fsync segment locally  
2. `publish` to cloud  
3. Atomic `manifest.bin` swing  
4. WAL checkpoint  
5. Compact: swing manifest → `remove` pruned keys locally and remotely  

Orphans on crash are harmless (unreferenced blobs).
