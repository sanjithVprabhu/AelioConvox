import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import { z } from 'zod';

const envRef = /\$\{([A-Z0-9_]+)\}/g;

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

export const DEFAULT_SDK_SECRET = 'change-me-in-production';

export const ConfigSchema = z
  .object({
  name: z.string().min(1),
  secret: z.string().min(1),
  llm: z
    .object({
      provider: z.enum(['anthropic', 'openai', 'gemini', 'groq', 'ollama', 'mock']),
      model: z.string().min(1),
      api_key: z.string().optional(),
      base_url: z.string().url().optional(),
      max_tokens: z.number().int().positive().default(4096),
      // Cheaper model for session summaries, daemon reflection, and other
      // background LLM work. Defaults to `model` when omitted.
      background_model: z.string().optional(),
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
    })
    .superRefine((value, ctx) => {
      if (!['mock', 'ollama'].includes(value.provider) && !value.api_key) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message: 'api_key is required unless provider is mock or ollama',
          path: ['api_key'],
        });
      }
    }),
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
        max_message_length: z.number().int().positive().default(4096),
        magic_link: z
          .object({
            enabled: z.boolean().default(true),
            ttl_minutes: z.number().int().positive().default(15),
            session_token_ttl_minutes: z.number().int().positive().default(60),
            // When true (default), POST /auth/magic-link requires the SDK secret.
            require_sdk_auth: z.boolean().default(true),
            // Per-IP and per-email issuance limits (sliding 1-minute window).
            rate_limit_per_minute: z.number().int().positive().default(5),
          })
          .default({
            enabled: true,
            ttl_minutes: 15,
            session_token_ttl_minutes: 60,
            require_sdk_auth: true,
            rate_limit_per_minute: 5,
          }),
      })
      .default({
        enabled: true,
        allowed_origins: [],
        max_message_length: 4096,
        magic_link: {
          enabled: true,
          ttl_minutes: 15,
          session_token_ttl_minutes: 60,
          require_sdk_auth: true,
          rate_limit_per_minute: 5,
        },
      }),
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
      enabled: z.boolean().default(false),
      router_model: z.string().optional(),
      planner_model: z.string().optional(),
      synthesis_model: z.string().optional(),
      plan_step_cap: z.number().int().positive().default(10),
      replan_cap: z.number().int().positive().default(3),
      token_budget: z.number().int().positive().default(50_000),
      wall_clock_ms: z.number().int().positive().default(60_000),
      flow_confidence_threshold: z.number().min(0).max(1).default(0.82),
      tool_retrieval_k: z.number().int().positive().default(12),
      force_categories: z
        .array(z.string())
        .default(['transaction', 'checkout', 'order', 'multi_step']),
      force_on_active_flow: z.boolean().default(true),
    })
    .default({
      enabled: false,
      plan_step_cap: 10,
      replan_cap: 3,
      token_budget: 50_000,
      wall_clock_ms: 60_000,
      flow_confidence_threshold: 0.82,
      tool_retrieval_k: 12,
      force_categories: ['transaction', 'checkout', 'order', 'multi_step'],
      force_on_active_flow: true,
    }),
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
      port: z.number().int().positive().default(3000),
    })
    .default({}),
})
  .superRefine((value, ctx) => {
    // AELIO_TEST_MODE skips production hard-fails so CI/docker-verify can use
    // the documented placeholder secret and local origins.
    const enforceProductionGuards =
      process.env.NODE_ENV === 'production' && process.env.AELIO_TEST_MODE !== '1';
    if (enforceProductionGuards && value.secret === DEFAULT_SDK_SECRET) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message:
          `Refusing to start with default SDK secret "${DEFAULT_SDK_SECRET}". ` +
          'Set AELIO_SDK_SECRET to a strong random value before deploying.',
        path: ['secret'],
      });
    }

    if (enforceProductionGuards && value.channels.web.enabled) {
      const origins = value.channels.web.allowed_origins.filter((origin) => origin.trim().length > 0);
      if (origins.length === 0 || origins.includes('*')) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message:
            'channels.web.allowed_origins must be an explicit non-wildcard list in production. ' +
            'Set AELIO_ALLOWED_ORIGINS (or list origins in config). "*" is dev-only.',
          path: ['channels', 'web', 'allowed_origins'],
        });
      }
    }

    if (
      enforceProductionGuards &&
      value.channels.whatsapp.enabled &&
      value.channels.whatsapp.provider === 'meta' &&
      !value.channels.whatsapp.mock_mode &&
      !value.channels.whatsapp.app_secret
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message:
          'channels.whatsapp.app_secret is required in production when provider is meta and mock_mode is false.',
        path: ['channels', 'whatsapp', 'app_secret'],
      });
    }

    if (
      value.sunjet.enabled &&
      value.embeddings.output_dimension != null &&
      value.embeddings.output_dimension !== value.sunjet.embed_dim
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: `embeddings.output_dimension (${value.embeddings.output_dimension}) must match sunjet.embed_dim (${value.sunjet.embed_dim})`,
        path: ['embeddings', 'output_dimension'],
      });
    }
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
  const resolved = resolveEnvDeep(raw) as Record<string, unknown>;

  // Allow comma-separated AELIO_ALLOWED_ORIGINS to populate web origins at deploy time.
  const envOrigins = process.env.AELIO_ALLOWED_ORIGINS?.split(',')
    .map((origin) => origin.trim())
    .filter(Boolean);
  if (envOrigins && envOrigins.length > 0) {
    const channels = (resolved.channels ?? {}) as Record<string, unknown>;
    const web = (channels.web ?? {}) as Record<string, unknown>;
    channels.web = { ...web, allowed_origins: envOrigins };
    resolved.channels = channels;
  }

  // Drop empty strings left by unresolved ${AELIO_ALLOWED_ORIGIN} placeholders.
  const channels = resolved.channels as Record<string, unknown> | undefined;
  const web = channels?.web as Record<string, unknown> | undefined;
  if (Array.isArray(web?.allowed_origins)) {
    web.allowed_origins = (web.allowed_origins as string[]).filter(
      (origin) => typeof origin === 'string' && origin.trim().length > 0,
    );
  }

  const result = ConfigSchema.safeParse(resolved);

  if (!result.success) {
    const details = result.error.issues
      .map((issue) => `  - ${issue.path.join('.') || '(root)'}: ${issue.message}`)
      .join('\n');
    throw new Error(`Invalid Aelio config at ${absolutePath}:\n${details}`);
  }

  return result.data;
}
