import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { z } from 'zod';

const envRef = /\$\{([A-Z0-9_]+)\}/g;

/** Default HTTP port — override with AELIO_PORT (or legacy AELIO_SERVER_PORT). */
export const DEFAULT_AELIO_PORT = 3010;

export function resolveAelioPort(yamlPort?: number): number {
  const raw = process.env.AELIO_PORT ?? process.env.AELIO_SERVER_PORT;
  if (raw !== undefined && raw !== '') {
    const port = Number(raw);
    if (Number.isInteger(port) && port > 0) return port;
  }
  return yamlPort ?? DEFAULT_AELIO_PORT;
}

function resolveEnvRefs(value: string): string {
  return value.replace(envRef, (_, name: string) => process.env[name] ?? '');
}

function resolveEnvDeep<T>(input: T): T {
  if (typeof input === 'string') {
    return resolveEnvRefs(input) as T;
  }
  if (Array.isArray(input)) {
    return input.map((item) => resolveEnvDeep(item)) as T;
  }
  if (input && typeof input === 'object') {
    return Object.fromEntries(
      Object.entries(input).map(([key, value]) => [key, resolveEnvDeep(value)]),
    ) as T;
  }
  return input;
}

export const ConfigSchema = z.object({
  name: z.string().min(1),
  secret: z.string().min(1),
  llm: z
    .object({
      provider: z.enum(['anthropic', 'openai', 'gemini', 'groq', 'ollama', 'mock']),
      model: z.string().min(1),
      api_key: z.string().optional(),
      base_url: z.string().url().optional(),
      max_tokens: z.number().int().positive().default(4096),
      // Assistant persona/voice. An SDK-registered persona (aelio.persona()) takes
      // precedence; this is the config-level fallback.
      system_prompt: z.string().optional(),
      fallback: z
        .object({
          provider: z.enum(['anthropic', 'openai', 'gemini', 'groq', 'ollama', 'mock']),
          model: z.string().min(1),
          api_key: z.string().optional(),
          base_url: z.string().url().optional(),
        })
        .optional(),
    }),
    // A missing api_key is intentionally NOT a schema error: the default provider
    // is OpenAI, and we want zero-config dev/test/demo to still boot. The server
    // degrades a keyless provider to the mock LLM (with a warning) in dev, and
    // hard-fails only in NODE_ENV=production — see resolveLlmChain in app.ts.
  channels: z.object({
    whatsapp: z
      .object({
        enabled: z.boolean(),
        // 'meta' = built-in Meta Cloud API adapter (default, turnkey).
        // 'sdk'  = bring-your-own provider: inbound via aelio.ingest(), delivery
        //          via the SDK's onSend handler. Aelio holds no provider creds.
        provider: z.enum(['meta', 'sdk']).default('meta'),
        mock_mode: z.boolean().default(false),
        phone_number_id: z.string().optional(),
        access_token: z.string().optional(),
        verify_token: z.string().optional(),
        app_secret: z.string().optional(),
        webhook_path: z.string().default('/wa/webhook'),
      })
      .default({ enabled: false, mock_mode: false }),
    web: z
      .object({
        enabled: z.boolean(),
        allowed_origins: z.array(z.string()).default([]),
        magic_link: z
          .object({
            enabled: z.boolean().default(true),
            ttl_minutes: z.number().int().positive().default(15),
            session_token_ttl_minutes: z.number().int().positive().default(60),
          })
          .default({ enabled: true, ttl_minutes: 15, session_token_ttl_minutes: 60 }),
      })
      .default({ enabled: true, allowed_origins: [], magic_link: { enabled: true, ttl_minutes: 15 } }),
  }),
  safety: z
    .object({
      default_mode: z.enum(['read_only', 'full']).default('read_only'),
      require_confirmation_for: z
        .array(z.enum(['read', 'write', 'destructive']))
        .default(['write', 'destructive']),
      overrides: z
        .record(
          z.object({
            mode: z.enum(['read', 'write', 'destructive', 'blocked']).optional(),
            require_confirmation: z.boolean().optional(),
            blocked: z.boolean().optional(),
          }),
        )
        .optional(),
      rate_limit: z
        .object({
          per_customer_per_minute: z.number().int().positive().default(20),
          per_customer_per_day: z.number().int().positive().default(500),
        })
        .default({}),
    })
    .default({}),
  identity: z
    .object({
      mapping_function: z.enum(['phone', 'email']).default('phone'),
      allow_anonymous: z.boolean().default(false),
    })
    .default({}),
  session: z
    .object({
      idle_timeout_minutes: z.number().int().positive().default(60),
      history_window: z.number().int().positive().default(20),
      summarize_after: z.number().int().positive().default(50),
    })
    .default({}),
  intent: z
    .object({
      enabled: z.boolean().default(true),
      ttl_minutes: z.number().int().positive().default(20),
      max_depth: z.number().int().positive().default(5),
    })
    .default({ enabled: true, ttl_minutes: 20, max_depth: 5 }),
  memory: z
    .object({
      enabled: z.boolean().default(true),
      recall_limit: z.number().int().positive().default(5),
    })
    .default({ enabled: true, recall_limit: 5 }),
  daemon: z
    .object({
      enabled: z.boolean().default(false),
      interval_minutes: z.number().int().positive().default(15),
      max_per_cycle: z.number().int().positive().default(5),
      reflect_min_messages: z.number().int().positive().default(2),
      // When true, an `unresolved` reflection auto-enqueues a proactive follow-up
      // (still gated by all proactive guardrails). Requires proactive.enabled.
      proactive_followup: z.boolean().default(false),
    })
    .default({
      enabled: false,
      interval_minutes: 15,
      max_per_cycle: 5,
      reflect_min_messages: 2,
      proactive_followup: false,
    }),
  proactive: z
    .object({
      enabled: z.boolean().default(false),
      require_opt_in: z.boolean().default(true),
      max_per_customer_per_day: z.number().int().positive().default(5),
      window_hours: z.number().int().positive().default(24),
    })
    .default({ enabled: false, require_opt_in: true, max_per_customer_per_day: 5, window_hours: 24 }),
  embeddings: z
    .object({
      // hash = built-in local (no API). openai | gemini | anthropic (Voyage) | ollama.
      provider: z.enum(['hash', 'openai', 'anthropic', 'gemini', 'ollama']).default('hash'),
      model: z.string().default('text-embedding-3-small'),
      api_key: z.string().optional(),
      base_url: z.string().url().optional(),
      // Match sunjet.embed_dim when using Sunjet vector columns (e.g. 1536 openai, 768 gemini).
      output_dimension: z.number().int().positive().optional(),
    })
    .default({ provider: 'hash', model: 'text-embedding-3-small' })
    .superRefine((value, ctx) => {
      if (!['hash', 'ollama'].includes(value.provider) && !value.api_key) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message: 'embeddings.api_key is required unless provider is hash or ollama',
          path: ['api_key'],
        });
      }
    }),
  cache: z
    .object({
      enabled: z.boolean().default(false),
      // High by default: only serve a cached reply to an essentially identical
      // question. Only no-tool replies are cached, so live data is never stale.
      similarity_threshold: z.number().min(0).max(1).default(0.92),
      ttl_minutes: z.number().int().positive().default(60),
    })
    .default({ enabled: false, similarity_threshold: 0.92, ttl_minutes: 60 }),
  harness: z
    .object({
      // The plan-execute-replan engine. false → legacy single-loop tool calling.
      enabled: z.boolean().default(true),
      budgets: z
        .object({
          max_instructions: z.number().int().positive().default(12),
          max_replans: z.number().int().nonnegative().default(2),
          max_recoils_per_intent: z.number().int().positive().default(3),
          max_tool_calls: z.number().int().positive().default(15),
          wall_clock_ms: z.number().int().positive().default(60_000),
          max_turn_tokens: z.number().int().positive().default(30_000),
        })
        .default({}),
      binding: z
        .object({
          score_min: z.number().min(0).max(1).default(0.55),
          ambiguity_gap: z.number().min(0).max(1).default(0.08),
          cache_ttl_minutes: z.number().int().positive().default(1440),
        })
        .default({}),
    })
    .default({}),
  sunjet: z
    .object({
      enabled: z.boolean().default(false),
      url: z.string().url().default('http://127.0.0.1:8080'),
      api_key: z.string().optional(),
      embed_dim: z.number().int().positive().default(1536),
      timeout_ms: z.number().int().positive().default(30_000),
      dual_write_sqlite: z.boolean().default(true),
      fallback_sqlite_on_error: z.boolean().default(true),
      tables: z
        .object({
          messages: z.string().min(1).default('convox_messages'),
          conversations: z.string().min(1).default('convox_conversations'),
          memories: z.string().min(1).default('convox_memories'),
          compactions: z.string().min(1).default('convox_compactions'),
          runtime_state: z.string().min(1).default('runtime_state'),
          harness_tools: z.string().min(1).default('harness_tools'),
          harness_capabilities: z.string().min(1).default('harness_capabilities'),
          harness_bindings: z.string().min(1).default('harness_bindings'),
          harness_suspensions: z.string().min(1).default('harness_suspensions'),
          harness_ledger: z.string().min(1).default('harness_ledger'),
          harness_traces: z.string().min(1).default('harness_traces'),
        })
        .default({}),
    })
    .default({}),
  storage: z.object({
    database_path: z.string().min(1),
    backup: z
      .object({
        enabled: z.boolean().default(true),
        interval_hours: z.number().int().positive().default(6),
        retain_count: z.number().int().positive().default(14),
      })
      .default({}),
  }),
  logging: z
    .object({
      level: z.enum(['debug', 'info', 'warn', 'error']).default('info'),
      format: z.enum(['json', 'pretty']).default('json'),
    })
    .default({}),
  server: z
    .object({
      host: z.string().default('0.0.0.0'),
      port: z.number().int().positive().default(DEFAULT_AELIO_PORT),
    })
    .default({}),
});

export type AelioConfig = z.infer<typeof ConfigSchema>;

const serverRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function resolveConfigPath(configPath: string): string {
  const candidates = [
    resolve(configPath),
    resolve(process.cwd(), configPath),
    resolve(serverRoot, configPath),
    resolve(serverRoot, '../..', configPath),
  ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  return resolve(configPath);
}

export function loadConfig(configPath = process.env.AELIO_CONFIG ?? './config.yaml'): AelioConfig {
  const absolutePath = resolveConfigPath(configPath);
  const raw = parse(readFileSync(absolutePath, 'utf8')) as unknown;
  const resolved = resolveEnvDeep(raw);
  const result = ConfigSchema.safeParse(resolved);

  if (!result.success) {
    const details = result.error.issues
      .map((issue) => `  - ${issue.path.join('.') || '(root)'}: ${issue.message}`)
      .join('\n');
    throw new Error(`Invalid Aelio config at ${absolutePath}:\n${details}`);
  }

  const config = result.data;
  config.server.port = resolveAelioPort(config.server.port);

  // Restrict widget origins at deploy time without rebuilding the image/config
  // (SEC-007): AELIO_WEB_ALLOWED_ORIGINS is a comma-separated allowlist that
  // overrides channels.web.allowed_origins. Set it in production so the baked
  // docker default (["*"]) can't leave the widget embeddable anywhere.
  const originsOverride = process.env.AELIO_WEB_ALLOWED_ORIGINS;
  if (originsOverride !== undefined) {
    config.channels.web.allowed_origins = originsOverride
      .split(',')
      .map((origin) => origin.trim())
      .filter((origin) => origin.length > 0);
  }

  return config;
}
