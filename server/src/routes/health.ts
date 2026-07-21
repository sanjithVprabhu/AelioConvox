import { existsSync } from 'node:fs';
import type { FastifyInstance, FastifyRequest } from 'fastify';
import type { RuntimeDeps } from '../runtime-deps.js';
import { resolvePublicDir } from '../paths.js';
import { bearerToken, secretsMatch } from '../auth.js';

export async function registerHealthRoutes(app: FastifyInstance, deps?: RuntimeDeps) {
  app.get('/health', async () => ({
    status: 'ok',
    service: 'aelio-server',
    version: process.env.AELIO_VERSION ?? '0.1.0',
    timestamp: new Date().toISOString(),
    uptimeSeconds: Math.floor(process.uptime()),
  }));

  app.get('/ready', async (request: FastifyRequest, reply) => {
    if (!deps) {
      return reply.status(503).send({ ready: false, reason: 'Server not initialized' });
    }

    const checks: Record<string, boolean> = {
      sunjet: false,
      widget: false,
    };

    // Aelio is Sunjet-only — readiness reflects Sunjet's own health directly.
    try {
      const health = await deps.sunjetClient.health();
      checks.sunjet = health.status === 'ok';
    } catch {
      checks.sunjet = false;
    }

    checks.widget = existsSync(resolvePublicDir() + '/widget.js');

    const ready = checks.widget && checks.sunjet;
    const isProduction = process.env.NODE_ENV === 'production';
    const authorized = secretsMatch(bearerToken(request), deps.config.secret);
    const redactCatalog = isProduction && !authorized;

    const body = {
      ready,
      checks,
      sdk: {
        connected: deps.sdkBridge.getFunctions().length > 0,
        ...(redactCatalog
          ? {}
          : {
              functions: deps.sdkBridge.getFunctions().map((fn) => fn.name),
              states: deps.sdkBridge.getStates().map((state) => state.id),
              policies: deps.sdkBridge.getPolicies().map((policy) => policy.id),
              flows: deps.sdkBridge.getFlows().map((flow) => flow.id),
            }),
      },
      memory: {
        enabled: deps.config.memory.enabled,
        backend: 'sunjet',
      },
      channels: {
        web: deps.config.channels.web.enabled,
        whatsapp: deps.config.channels.whatsapp.enabled,
      },
      llm: deps.config.llm.provider,
      sunjet: {
        enabled: deps.config.sunjet.enabled,
        url: deps.config.sunjet.url,
        messageBackend: 'sunjet',
        memoryBackend: 'sunjet',
        sessionBackend: 'sunjet',
        customerBackend: 'sunjet',
        jobBackend: 'sunjet',
        responseCacheBackend: 'sunjet',
        functionCallBackend: 'sunjet',
        reflectionBackend: 'sunjet',
        proactiveBackend: 'sunjet',
        inboundDedupBackend: 'sunjet',
        magicLinkBackend: 'sunjet',
        sdkConnectionBackend: 'sunjet',
        segmentStorage: {
          backend: deps.config.sunjet.segment_storage.backend,
          prefix: deps.config.sunjet.segment_storage.prefix ?? null,
          bucket: deps.config.sunjet.segment_storage.bucket ?? null,
          region: deps.config.sunjet.segment_storage.region ?? null,
          endpoint: deps.config.sunjet.segment_storage.endpoint ?? null,
          // Secrets never echoed — only whether they are set.
          credentialsConfigured: Boolean(
            process.env.AELIO_SUNJET_S3_ACCESS_KEY_ID ||
              process.env.LL_S3_ACCESS_KEY_ID ||
              process.env.AWS_ACCESS_KEY_ID,
          ),
        },
      },
    };

    return reply.status(ready ? 200 : 503).send(body);
  });

  if (process.env.AELIO_TEST_MODE === '1' || process.env.AELIO_DIAGNOSTICS === '1') {
    app.get('/diagnostics', async () => {
      const mem = process.memoryUsage();
      return {
        status: 'ok',
        service: 'aelio-server',
        version: process.env.AELIO_VERSION ?? '0.1.0',
        node: process.version,
        pid: process.pid,
        uptimeSeconds: Math.floor(process.uptime()),
        memory: {
          rssMb: Math.round(mem.rss / 1024 / 1024),
          heapUsedMb: Math.round(mem.heapUsed / 1024 / 1024),
        },
        paths: {
          cwd: process.cwd(),
          public: resolvePublicDir(),
        },
        config: deps
          ? {
              name: deps.config.name,
              llm: deps.config.llm.provider,
              safetyMode: deps.config.safety.default_mode,
              memoryEnabled: deps.config.memory.enabled,
              webEnabled: deps.config.channels.web.enabled,
              whatsappEnabled: deps.config.channels.whatsapp.enabled,
            }
          : null,
        sdk: deps
          ? {
              functions: deps.sdkBridge.getFunctions().map((fn) => ({
                name: fn.name,
                safety: fn.safety,
              })),
            }
          : null,
      };
    });
  }
}
