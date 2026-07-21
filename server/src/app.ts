import { MetaWhatsAppSender, MockWhatsAppSender } from '@aelio/channels';
import { createLLMProviderChain, createEmbeddingProvider, type LLMProviderConfig } from '@aelio/llm';
import {
  configureEmbedder,
  createImmediateContextEngine,
  createSemanticPathwayEngine,
  createArchetypeEngine,
  seedBuiltinArchetypes,
  DEFAULT_ARCHETYPES,
  createInstrumentedLlm,
  HarnessTracer,
  LighthouseService,
  SuspensionStore,
} from '@aelio/core';
import websocket from '@fastify/websocket';
import fastifyStatic from '@fastify/static';
import Fastify from 'fastify';
import type { AelioConfig } from './config.js';
import { resolvePublicDir } from './paths.js';
import { registerAuthRoutes } from './routes/auth.js';
import { registerHealthRoutes } from './routes/health.js';
import { registerSdkRoutes } from './routes/sdk.js';
import { registerWhatsAppRoutes } from './routes/whatsapp.js';
import { registerTestRoutes } from './routes/test.js';
import { registerWidgetRoutes } from './routes/widget.js';
import { registerProactiveRoutes } from './routes/proactive.js';
import { registerTelemetryRoutes } from './routes/telemetry.js';
import { registerAdminDbRoutes } from './routes/admin-db.js';
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
  const publicDir = resolvePublicDir();

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

  // Aelio is Sunjet-only: every Convox store (messages, memories, sessions,
  // customers, jobs, ledger, etc.) lives on Sunjet/Astrolobe. There is no
  // SQLite fallback — a Sunjet outage at boot is fatal, not degraded service.
  config.sunjet.enabled = true;
  const sunjet = await initSunjet(config);

  const sdkBridge = new ServerSdkBridge(sunjet.sdkConnectionStore);

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
  // changes re-hash immediately; the Sunjet mirror resyncs in the background,
  // keyed by that hash.
  const lighthouse = new LighthouseService({
    bridge: sdkBridge,
    tenant: config.name,
    sunjet: {
      client: sunjet.client,
      toolsTable: sunjet.tables.harnessTools,
      capabilitiesTable: sunjet.tables.harnessCapabilities,
      embedDim: sunjet.embedDim,
    },
  });
  sdkBridge.onRegistryChange(() => lighthouse.refresh());

  // Harness trace firehose — Sunjet-only, lossy-tolerant.
  const tracer = new HarnessTracer({
    client: sunjet.client,
    table: sunjet.tables.harnessTraces,
    tenant: config.name,
    embedDim: sunjet.embedDim,
  });

  // Suspended-plan store: Sunjet is the sole source of truth (see SuspensionStore docs).
  const suspensionStore = new SuspensionStore({
    sunjet: { client: sunjet.client, table: sunjet.tables.harnessSuspensions, tenant: config.name },
  });

  // Immediate Context Engine: time-bucketed short-term context per customer
  // (hot 5-min window cached in-process, older windows compacted into Sunjet).
  const contextEngine = createImmediateContextEngine({
    storage: sunjet.storageConfig,
    llm,
    model: config.llm.model,
  });
  const pathwayEngine = createSemanticPathwayEngine({
    memoryStore: sunjet.memoryStore,
  });

  // Archetype valence engine + self-learning aspect taxonomy. The mother
  // collection (aspectStore) registers the "things" we track; each owns a
  // pos/neu/neg bucket set. Seed the builtin aspects once (a tenant can add its
  // own); seeding is best-effort so a slow/unavailable embedder never blocks
  // startup. Discovery of new aspects happens post-turn via engine.learn().
  const archetypeEngine = createArchetypeEngine({
    store: sunjet.archetypeStore,
    aspectStore: sunjet.aspectStore,
  });
  void seedBuiltinArchetypes(sunjet.aspectStore, sunjet.archetypeStore, DEFAULT_ARCHETYPES)
    .then((count) => {
      if (count > 0) {
        console.log(`[aelio] seeded ${count} builtin aspects with valence buckets`);
      }
    })
    .catch((error) => {
      console.warn(
        `[aelio] archetype seeding skipped: ${error instanceof Error ? error.message : String(error)}`,
      );
    });

  const deps: RuntimeDeps = {
    config,
    llm,
    sdkBridge,
    lighthouse,
    tracer,
    suspensionStore,
    whatsappSender,
    sunjetClient: sunjet.client,
    messageStore: sunjet.messageStore,
    memoryStore: sunjet.memoryStore,
    sessionStore: sunjet.sessionStore,
    customerStore: sunjet.customerStore,
    jobStore: sunjet.jobStore,
    responseCacheStore: sunjet.responseCacheStore,
    functionCallStore: sunjet.functionCallStore,
    reflectionStore: sunjet.reflectionStore,
    proactiveStore: sunjet.proactiveStore,
    inboundDedupStore: sunjet.inboundDedupStore,
    magicLinkStore: sunjet.magicLinkStore,
    sdkConnectionStore: sunjet.sdkConnectionStore,
    contextEngine,
    pathwayEngine,
    archetypeEngine,
    axisStore: sunjet.axisStore,
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
  await registerAdminDbRoutes(app, deps);
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
  });

  return { app, deps, publicDir };
}
