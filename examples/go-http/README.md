# Go HTTP example — connect a tiny Go backend to Aelio

```bash
# Terminal 1 — Aelio server (Docker or local)
docker run -d -p 3010:3000 -e AELIO_SDK_SECRET=change-me aelio/server:latest

# Terminal 2 — this example
cd examples/go-http
export AELIO_SDK_SECRET=change-me
export AELIO_SERVER_URL=ws://127.0.0.1:3010
go run .
```
