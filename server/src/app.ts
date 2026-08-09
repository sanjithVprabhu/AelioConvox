import { MetaWhatsAppSender, MockWhatsAppSender } from '@aelio/channels';
import { createLLMProviderChain, createEmbeddingProvider, type LLMProviderConfig } from '@aelio/llm';
import {
  configureEmbedder,
  createInstrumentedLlm,
} from '@aelio/core/edge';
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
import { registerAelioHostRoutes } from './routes/aelio-host.js';
import { registerAelioGatewayRoutes } from './routes/aelio-gateway.js';
import type { RuntimeDeps } from './runtime-deps.js';
import { ServerSdkBridge } from './sdk-bridge.js';
import { initAelioDb } from './aelio-db.js';
import { startBackupWorker } from './workers/backup.js';
import { startAelioJobWorker } from './workers/aelio-jobs.js';
import { startInboundWorker } from './workers/inbound.js';
import { startOutboundWorker } from './workers/outbound.js';
import { AelioRuntimeClient } from './aelio-runtime-client.js';
import { buildAgentCatalog } from './aelio-agent-catalog.js';
import { createQuietLoggerStream } from './turn-pipeline-log.js';
import { hydrateCatalogBagFromStore } from './catalog-bag.js';

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
      outputDimension: config.embeddings.output_dimension ?? config.aelioDb.embed_dim,
      timeoutMs: config.embeddings.timeout_ms,
    });
    configureEmbedder((text) => embeddingProvider.embed(text));
  } else {
    // createApp is also used repeatedly in tests. Do not retain a provider from
    // an earlier app instance through the process-global embedder hook.
    configureEmbedder(null);
  }

  // Every Convox store lives in the Rust Aelio database. There is no SQLite
  // fallback: database unavailability at boot is fatal.
  config.aelioDb.enabled = true;
  const aelioDb = await initAelioDb(config);

  // Hot catalog bag: active tools/states/policies/flows always in process memory.
  try {
    const bag = await hydrateCatalogBagFromStore(aelioDb.catalogEntityStore, config.name);
    console.info(
      `[aelio] catalog bag hydrated — tools=${bag.tools.length} states=${bag.states.length} ` +
        `flows=${bag.flows.length} policies=${bag.policies.length}`,
    );
  } catch (error) {
    console.warn('[aelio] catalog bag hydrate skipped:', error);
  }

  const sdkBridge = new ServerSdkBridge(aelioDb.sdkConnectionStore);
  const runtimeUrl = process.env.AELIO_RUST_RUNTIME_URL;
  const runtimeToken = process.env.AELIO_RUNTIME_TOKEN;
  if (!runtimeUrl || !runtimeToken) {
    throw new Error(
      'AELIO_RUST_RUNTIME_URL and AELIO_RUNTIME_TOKEN are required; TypeScript is not an execution authority',
    );
  }
  const aelioRuntime = new AelioRuntimeClient(runtimeUrl, runtimeToken);
  await aelioRuntime.ready();
  await aelioRuntime.pushAgentCatalog(buildAgentCatalog(config.name, {
    type: 'register',
    sdkVersion: 'server-bootstrap',
    language: 'node',
    functions: [],
    states: [],
    policies: [],
    flows: [],
    persona: config.llm.system_prompt,
    productBrief: `Aelio tenant ${config.name}`,
  }, { memoryEnabled: config.memory.enabled }));

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

  const deps: RuntimeDeps = {
    config,
    llm,
    sdkBridge,
    aelioRuntime,
    whatsappSender,
    aelioDbClient: aelioDb.client,
    messageStore: aelioDb.messageStore,
    sessionStore: aelioDb.sessionStore,
    memoryStore: aelioDb.memoryStore,
    customerStore: aelioDb.customerStore,
    jobStore: aelioDb.jobStore,
    inboundDedupStore: aelioDb.inboundDedupStore,
    magicLinkStore: aelioDb.magicLinkStore,
    sdkConnectionStore: aelioDb.sdkConnectionStore,
    catalogEntityStore: aelioDb.catalogEntityStore,
  };

  const app = Fastify({
    logger: {
      level: config.logging.level,
      stream: createQuietLoggerStream(),
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
  await registerAelioHostRoutes(app, deps);
  await registerAelioGatewayRoutes(app, deps);
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

  const stopInbound = startInboundWorker(deps, app.log);
  const stopOutbound = startOutboundWorker(deps, app.log);
  const stopBackup = startBackupWorker(deps);
  const stopAelioJobs = startAelioJobWorker(deps, app.log);

  app.addHook('onClose', async () => {
    stopInbound();
    stopOutbound();
    stopBackup();
    stopAelioJobs();
    sdkBridge.shutdown();
  });

  return { app, deps, publicDir };
}
