import { createDatabase } from '@aelio/db';
import { MetaWhatsAppSender, MockWhatsAppSender } from '@aelio/channels';
import { createLLMProviderChain, createEmbeddingProvider } from '@aelio/llm';
import { configureEmbedder } from '@aelio/core';
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
import type { RuntimeDeps } from './runtime-deps.js';
import { ServerSdkBridge } from './sdk-bridge.js';
import { startBackupWorker } from './workers/backup.js';
import { startDaemonWorker } from './workers/daemon.js';
import { startInboundWorker } from './workers/inbound.js';
import { startOutboundWorker } from './workers/outbound.js';

export async function createApp(config: AelioConfig) {
  const migrationsFolder = resolveMigrationsFolder();
  const publicDir = resolvePublicDir();

  const database = createDatabase(config.storage.database_path);
  database.migrate(migrationsFolder);

  const llm = createLLMProviderChain([
    {
      provider: config.llm.provider,
      model: config.llm.model,
      apiKey: config.llm.api_key,
      maxTokens: config.llm.max_tokens,
      baseUrl: config.llm.base_url,
    },
    ...(config.llm.fallback
      ? [
          {
            provider: config.llm.fallback.provider,
            model: config.llm.fallback.model,
            apiKey: config.llm.fallback.api_key,
            maxTokens: config.llm.max_tokens,
            baseUrl: config.llm.fallback.base_url,
          },
        ]
      : []),
  ]);

  // Wire a real embedding model if configured; otherwise the built-in hash
  // embedding stays in use. Failures at call time fall back to the hash.
  if (config.embeddings.provider !== 'hash') {
    const embeddingProvider = createEmbeddingProvider({
      provider: config.embeddings.provider,
      model: config.embeddings.model,
      apiKey: config.embeddings.api_key,
      baseUrl: config.embeddings.base_url,
    });
    configureEmbedder((text) => embeddingProvider.embed(text));
  }

  const sdkBridge = new ServerSdkBridge(database);

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
    database,
    llm,
    sdkBridge,
    whatsappSender,
  };

  const app = Fastify({
    logger: {
      level: config.logging.level,
    },
  });

  await app.register(websocket);
  await registerHealthRoutes(app, deps);
  await registerSdkRoutes(app, deps);
  await registerAuthRoutes(app, deps);
  await registerWidgetRoutes(app, deps);
  await registerWhatsAppRoutes(app, deps);
  await registerProactiveRoutes(app, deps);
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
    database.close();
  });

  return { app, database, deps, publicDir, migrationsFolder };
}
