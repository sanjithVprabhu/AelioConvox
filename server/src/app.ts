import { createDatabase } from '@aelio/db';
import { MetaWhatsAppSender, MockWhatsAppSender } from '@aelio/channels';
import { createLLMProviderChain, createEmbeddingProvider, type LLMProviderConfig } from '@aelio/llm';
import {
  configureEmbedder,
  createInstrumentedLlm,
  HarnessTracer,
  LighthouseService,
  SuspensionStore,
} from '@aelio/core';
import websocket from '@fastify/websocket';
import fastifyStatic from '@fastify/static';
import Fastify from 'fastify';
import type { AelioConfig } from './config.js';
import { resolveMigrationsFolder, resolvePublicDir } from './paths.js';
import { registerAuthRoutes } from './routes/auth.js';
import { registerHealthRoutes } from './routes/health.js';
import { registerSdkRoutes } from './routes/sdk.js';
import { registerWhatsAppRoutes } from './routes/whatsapp.js';
import { registerTestRoutes } from './routes/test.js';
import { registerWidgetRoutes } from './routes/widget.js';
import { registerProactiveRoutes } from './routes/proactive.js';
import { registerTelemetryRoutes } from './routes/telemetry.js';
import type { RuntimeDeps } from './runtime-deps.js';
import { ServerSdkBridge } from './sdk-bridge.js';
import { initSunjet } from './sunjet.js';
import { startBackupWorker } from './workers/backup.js';
import { startDaemonWorker } from './workers/daemon.js';
import { startInboundWorker } from './workers/inbound.js';
import { startOutboundWorker } from './workers/outbound.js';

/** Providers that don't need an API key (they run locally / are test doubles). */
const KEYLESS_PROVIDERS = new Set(['mock', 'ollama']);

/**
 * Turn the config's LLM section into a provider chain, degrading gracefully.
 * The default provider is OpenAI; a keyless real provider would otherwise crash
 * zero-config dev / test / demo, so a provider whose api_key is empty degrades
 * to the mock LLM with a loud warning. A deployment that wants a missing key to
 * be fatal (never silently serve mock to real users) sets AELIO_REQUIRE_LLM_KEY=1.
 */
export function resolveLlmChain(config: AelioConfig): [LLMProviderConfig, ...LLMProviderConfig[]] {
  const requireKey = process.env.AELIO_REQUIRE_LLM_KEY === '1';

  const resolve = (
    p: { provider: AelioConfig['llm']['provider']; model: string; api_key?: string; base_url?: string },
    role: 'primary' | 'fallback',
  ): LLMProviderConfig => {
    const needsKey = !KEYLESS_PROVIDERS.has(p.provider);
    if (needsKey && !p.api_key) {
      if (requireKey) {
        throw new Error(
          `LLM ${role} provider "${p.provider}" is configured but its api_key is empty, ` +
            `and AELIO_REQUIRE_LLM_KEY=1. Set the provider's API key (e.g. OPENAI_API_KEY).`,
        );
      }
      console.warn(
        `[aelio] LLM ${role} provider "${p.provider}" has no api_key — using the mock LLM instead. ` +
          `Set the key (e.g. OPENAI_API_KEY) for real responses.`,
      );
      return { provider: 'mock', model: p.model };
    }
    return {
      provider: p.provider,
      model: p.model,
      apiKey: p.api_key,
      maxTokens: config.llm.max_tokens,
      baseUrl: p.base_url,
    };
  };

  const primary = resolve(config.llm, 'primary');
  // If the primary degraded to mock, a fallback is unreachable (mock never
  // throws) — skip it to avoid a second confusing warning.
  if (primary.provider === 'mock' || !config.llm.fallback) {
    return [primary];
  }
  return [primary, resolve(config.llm.fallback, 'fallback')];
}

export async function createApp(config: AelioConfig) {
  const migrationsFolder = resolveMigrationsFolder();
  const publicDir = resolvePublicDir();

  const database = createDatabase(config.storage.database_path);
  database.migrate(migrationsFolder);

  const llm = createInstrumentedLlm(createLLMProviderChain(resolveLlmChain(config)));

  // Wire a real embedding model if configured; otherwise the built-in hash
  // embedding stays in use. Failures at call time fall back to the hash.
  if (config.embeddings.provider !== 'hash') {
    const embeddingProvider = createEmbeddingProvider({
      provider: config.embeddings.provider,
      model: config.embeddings.model,
      apiKey: config.embeddings.api_key,
      baseUrl: config.embeddings.base_url,
      outputDimension: config.embeddings.output_dimension ?? config.sunjet.embed_dim,
    });
    configureEmbedder((text) => embeddingProvider.embed(text));
  }

  const sdkBridge = new ServerSdkBridge(database);

  // A Sunjet outage at boot must not crash-loop the server when the config
  // allows SQLite fallback — conversations keep working, archival degrades.
  let sunjet: Awaited<ReturnType<typeof initSunjet>> = null;
  try {
    sunjet = await initSunjet(config);
  } catch (error) {
    if (!config.sunjet.enabled || !config.sunjet.fallback_sqlite_on_error) {
      throw error;
    }
    console.error(
      '[aelio] Sunjet unavailable at startup — continuing on SQLite only:',
      error instanceof Error ? error.message : String(error),
    );
  }

  const whatsapp = config.channels.whatsapp;
  // provider: 'sdk' means the dev delivers outbound themselves via onSend, so we
  // build no built-in sender — the outbound worker routes through the SDK bridge.
  const whatsappSender =
    !whatsapp.enabled || whatsapp.provider === 'sdk'
      ? null
      : whatsapp.mock_mode
        ? new MockWhatsAppSender()
        : whatsapp.phone_number_id && whatsapp.access_token
          ? new MetaWhatsAppSender({
              phoneNumberId: whatsapp.phone_number_id,
              accessToken: whatsapp.access_token,
            })
          : new MockWhatsAppSender();

  // Lighthouse: the harness's read model over the SDK registry. Registry
  // changes re-hash immediately; the Sunjet mirror (when available) resyncs
  // in the background, keyed by that hash.
  const lighthouse = new LighthouseService({
    bridge: sdkBridge,
    tenant: config.name,
    ...(sunjet
      ? {
          sunjet: {
            client: sunjet.client,
            toolsTable: sunjet.tables.harnessTools,
            capabilitiesTable: sunjet.tables.harnessCapabilities,
            embedDim: sunjet.embedDim,
          },
        }
      : {}),
  });
  sdkBridge.onRegistryChange(() => lighthouse.refresh());

  // Harness trace firehose — Sunjet-only, lossy-tolerant; null without Sunjet.
  const tracer = sunjet
    ? new HarnessTracer({
        client: sunjet.client,
        table: sunjet.tables.harnessTraces,
        tenant: config.name,
        embedDim: sunjet.embedDim,
      })
    : null;

  // Suspended-plan store: SQLite is authoritative (always present); Sunjet
  // mirrors when available so parked plans are visible in Astrolobe too.
  const suspensionStore = new SuspensionStore({
    database,
    ...(sunjet
      ? { sunjet: { client: sunjet.client, table: sunjet.tables.harnessSuspensions, tenant: config.name } }
      : {}),
  });

  const deps: RuntimeDeps = {
    config,
    database,
    llm,
    sdkBridge,
    lighthouse,
    tracer,
    suspensionStore,
    whatsappSender,
    sunjetClient: sunjet?.client ?? null,
    messageStore: sunjet?.messageStore ?? null,
  };

  const app = Fastify({
    logger: {
      level: config.logging.level,
    },
  });

  // Capture the exact request bytes on JSON parse so webhook HMAC verification
  // (WhatsApp) can sign the RAW body — Meta signs the bytes it sent, and
  // re-serializing via JSON.stringify would not byte-match (SEC-004).
  app.addContentTypeParser(
    'application/json',
    { parseAs: 'string' },
    (req, body, done) => {
      (req as unknown as { rawBody?: string }).rawBody =
        typeof body === 'string' ? body : Buffer.from(body).toString('utf8');
      const text = typeof body === 'string' ? body : Buffer.from(body).toString('utf8');
      if (text.trim() === '') {
        done(null, {});
        return;
      }
      try {
        done(null, JSON.parse(text));
      } catch (error) {
        done(error as Error, undefined);
      }
    },
  );

  await app.register(websocket);
  await registerHealthRoutes(app, deps);
  await registerSdkRoutes(app, deps);
  await registerAuthRoutes(app, deps);
  await registerWidgetRoutes(app, deps);
  await registerWhatsAppRoutes(app, deps);
  await registerProactiveRoutes(app, deps);
  await registerTelemetryRoutes(app, deps);
  await registerTestRoutes(app, sdkBridge);
  await app.register(fastifyStatic, {
    root: publicDir,
    prefix: '/',
    decorateReply: false,
  });

  const stopInbound = startInboundWorker(deps);
  const stopOutbound = startOutboundWorker(deps);
  const stopBackup = startBackupWorker(deps);
  const stopDaemon = startDaemonWorker(deps);

  app.addHook('onClose', async () => {
    stopInbound();
    stopOutbound();
    stopBackup();
    stopDaemon();
    sdkBridge.shutdown();
    database.close();
  });

  return { app, database, deps, publicDir, migrationsFolder };
}
