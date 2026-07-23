# @aelio/llm-gateway

A **stateless multi-LLM gateway** — the "modem" the Rust brain dials when an ability
(`Express.Synthesize`, `Learn.ProposePath`, an escalated `Understand.*`) needs a model.

```
Rust (aelio-server / DurableRuntime)      TypeScript (@aelio/llm-gateway)
  decides IF a model is needed
  builds PromptSpec + closed schema
        │  POST /v1/llm/complete  ─────────▶  picks vendor (OpenAI / Anthropic / Gemini / …)
        │                                     runs the call, shapes a unified reply
        ◀── { content, usage, provider } ──   returns raw text/JSON
  parses/validates vs the SAME closed schema
  continues Bind / Invoke / Express / Learn
```

**Rust decides when and why. TypeScript only executes the model call.**

## What this owns
- Multi-provider SDKs, vendor auth, retries, provider quirks (via `@aelio/llm`).
- Model routing (`tier:cheap` / `tier:smart` / concrete ids → a vendor + model).
- Vendor API keys (the gateway's own env — **never** tenant product keys).

## What this must never own
Sense · Understand routing · FlowGate · Policy · Bind · Invoke · Once · Park ·
tool calling into the SaaS product · tier selection · promotion · "which flow am I in".
If the gateway started choosing tools or flows, you'd be back to LLM-as-router.

## Endpoint

```
POST /v1/llm/complete
{
  "request_id": "aelio-…",                 // idempotency key; identical ids dedup within a TTL
  "model": "tier:cheap",                    // or a concrete id: "gpt-4o-mini", "claude-sonnet-5"
  "temperature": 0,
  "prompt": { "text": "…rendered prompt…", "spec_id": "…", "prompt_hash": "…" },
  "response_format": {                      // optional; when present the reply is forced to it
    "type": "json_schema",
    "json_schema": { "name": "aelio_synthesize", "strict": true, "schema": { … } }
  }
}
→
{
  "content": "{…}",                          // structured-output tool args (JSON) or free text
  "usage": { "input_tokens": 12, "output_tokens": 5 },
  "provider": "openai",
  "model": "gpt-4o-mini",
  "request_id": "aelio-…"
}
```

`GET /healthz` → `{ "ok": true }`.

Structured output is produced by forcing a single tool whose `input_schema` **is** the
closed schema Rust sent — the same mechanism `@aelio/llm` uses everywhere — so it works
uniformly across Anthropic, OpenAI, and Gemini. Rust re-validates the `content` against
that schema and has the final say; the model has no free-form escape.

## Guardrails
- **Completion only.** Caller-supplied `tools` / `tool_choice` are rejected (400).
- **Closed output only.** The gateway forces the schema; Rust re-validates.
- **Idempotency.** `request_id` dedups within `LLM_GATEWAY_IDEMPOTENCY_TTL_MS` (default 60s).
- **No secrets in logs.** Access logs carry `request_id` / `spec_id` / `prompt_hash` / provider — never prompt text.
- **One contract for every vendor.** Same request/response shape regardless of who answered.

## Configuration (env)
| Var | Purpose | Default |
|---|---|---|
| `PORT` / `HOST` | listen address | `8787` / `0.0.0.0` |
| `LLM_GATEWAY_TOKEN` | optional bearer token to authenticate callers (the gateway's own token) | none (open in dev) |
| `LLM_GATEWAY_DEFAULT_PROVIDER` | provider for an unrecognised bare model | `openai` |
| `LLM_TIER_CHEAP` / `LLM_TIER_SMART` | concrete model for each tier alias | `gpt-4o-mini` / `claude-sonnet-5` |
| `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `GEMINI_API_KEY` / `GROQ_API_KEY` | vendor keys | — |
| `LLM_GATEWAY_IDEMPOTENCY_TTL_MS` | dedup window | `60000` |

## Run
```bash
pnpm --filter @aelio/llm-gateway dev        # tsx watch, http://localhost:8787
pnpm --filter @aelio/llm-gateway test       # node:test, no network/keys needed
pnpm --filter @aelio/llm-gateway build      # dist/
```

## Rust side
Production wires `aelio::provider::TsGatewayProvider` (env: `AELIO_LLM_GATEWAY_URL`,
optional `AELIO_LLM_GATEWAY_TOKEN`, `AELIO_LLM_MODEL`) behind the existing `LlmProvider`
trait, so abilities are unchanged. Tests keep using `ScriptedLlmProvider`; a direct
`OpenAiCompatibleProvider` remains available for a no-gateway path.
