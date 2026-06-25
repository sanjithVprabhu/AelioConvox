# Production Testing Checklist

## Before client trials

1. Set `llm.provider` to `openai`, `anthropic`, `groq`, or `ollama`.
2. Set `llm.fallback` to a second provider for resilience.
3. Set a strong `secret`.
4. Review `channels.web.allowed_origins`.
5. Configure WhatsApp credentials if using WhatsApp.
6. Enable backups and confirm files appear next to the SQLite database in `backups/`.
7. Run `pnpm build` and `pnpm test:all`.

## Recommended config choices

- `safety.default_mode: read_only` for first external trials.
- Keep write actions confirmation-enabled.
- Lower `session.summarize_after` if chats are long.
- Keep `memory.enabled: true` and tune `memory.recall_limit`.

## Trial flow

1. Start the server.
2. Start one SDK integration.
3. Verify `/health`, `/ready`, and `/diagnostics`.
4. Test the web widget first.
5. Then test WhatsApp with a sandbox number or template-approved number.
