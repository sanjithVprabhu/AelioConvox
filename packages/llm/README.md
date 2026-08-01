# @aelio/llm

The single place all LLM and embedding provider logic lives. Consumers import
only from the package root (`@aelio/llm`); everything else is an implementation
detail behind the factories.

## Layout

```
src/
├── index.ts          Public API + the createLLMProvider / createLLMProviderChain factories
├── types.ts          Shared interface: LLMProvider, ChatMessage, ToolDefinition, LLMToolChoice
├── providers/        One file per chat provider — no cross-talk between them
│   ├── anthropic.ts        Claude (native /v1/messages, tool_choice)
│   ├── openai.ts           GPT (thin wrapper over openai-compatible)
│   ├── openai-compatible.ts Shared OpenAI-style chat/completions mapping
│   ├── gemini.ts           Gemini (generateContent, functionCallingConfig)
│   ├── groq.ts / ollama.ts OpenAI-compatible endpoints
│   ├── mock.ts             Deterministic test double (keyword heuristics + forced-tool)
│   ├── scripted.ts         FIFO scripted results for unit tests
│   ├── fallback.ts         Primary → fallback chain (first success wins)
│   └── index.ts            Barrel
└── embeddings/       Embedding providers behind one factory
    ├── index.ts            createEmbeddingProvider (openai · anthropic/voyage · gemini · ollama)
    └── anthropic.ts        Voyage-backed embeddings (Anthropic has no native embed API)
```

## Choosing a provider — purely config-driven

Any of the three first-class chat providers is selected by config alone; the
runtime never hardcodes one. Ship-ready profiles live at the repo root:

| Provider | Config | Env var | Chat model | Embeddings |
|----------|--------|---------|------------|-----------|
| **Anthropic (Claude)** | `config.anthropic.yaml` | `ANTHROPIC_API_KEY` (+ `VOYAGE_API_KEY` for embeddings) | `claude-sonnet-4-6` | Voyage `voyage-3-lite` (1536d) |
| **OpenAI (GPT)** | `config.openai.yaml` | `OPENAI_API_KEY` | `gpt-4o-mini` | `text-embedding-3-small` (1536d) |
| **Gemini** | `config.gemini.yaml` | `GEMINI_API_KEY` | `gemini-2.0-flash` | `text-embedding-004` (768d) |

```bash
# Run the server against any provider by pointing AELIO_CONFIG at its profile:
AELIO_CONFIG="$(pwd)/config.openai.yaml" OPENAI_API_KEY=sk-... pnpm --filter @aelio/server dev
```

A `fallback:` block adds a second provider that takes over on the primary's
failure (see `config.anthropic.yaml`).

> **Embedding dimensions must match storage.** When you switch providers, keep
> `embeddings.output_dimension` and `aelioDb.embed_dim` aligned (1536 for
> OpenAI/Voyage, 768 for Gemini) or vector recall silently degrades.

## Adding a provider

1. Add `src/providers/<name>.ts` implementing `LLMProvider` (map `toolChoice`
   for forced structured output — the harness relies on it).
2. Export it from `src/providers/index.ts`.
3. Add a `case` to `createLLMProvider` and the name to `SUPPORTED_LLM_PROVIDERS`.

## Tests

`pnpm test:llm` — constructs every provider + embedding provider from config,
asserts missing keys fail loudly, and checks the shipped configs. No network.
