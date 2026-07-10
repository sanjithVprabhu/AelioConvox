import { existsSync } from 'node:fs';
import type { FastifyInstance } from 'fastify';
import { authorized } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';
import { resolveMigrationsFolder, resolvePublicDir } from '../paths.js';

export async function registerHealthRoutes(app: FastifyInstance, deps?: RuntimeDeps) {
  app.get('/health', async () => ({
    status: 'ok',
    service: 'aelio-server',
    version: process.env.AELIO_VERSION ?? '0.1.0',
    timestamp: new Date().toISOString(),
    uptimeSeconds: Math.floor(process.uptime()),
  }));

  app.get('/ready', async (request, reply) => {
    if (!deps) {
      return reply.status(503).send({ ready: false, reason: 'Server not initialized' });
    }

    const checks: Record<string, boolean> = {
      database: false,
      migrations: false,
      widget: false,
      sunjet: !deps.config.sunjet.enabled,
    };

    try {
      deps.database.sqlite.prepare('SELECT 1').get();
      checks.database = true;
    } catch {
      checks.database = false;
    }

    checks.migrations = existsSync(resolveMigrationsFolder());
    checks.widget = existsSync(resolvePublicDir() + '/widget.js');

    if (deps.config.sunjet.enabled) {
      try {
        if (deps.sunjetClient) {
          const health = await deps.sunjetClient.health();
          checks.sunjet = health.status === 'ok';
        } else {
          checks.sunjet = false;
        }
      } catch {
        checks.sunjet = false;
      }
    }

    const ready = checks.database && checks.migrations && checks.widget && checks.sunjet;
    const sdkConnected = deps.sdkBridge.getFunctions().length > 0;
    const exposeSdkCatalog =
      process.env.NODE_ENV !== 'production' ||
      process.env.AELIO_TEST_MODE === '1' ||
      authorized(request, deps.config.secret);

    const body = {
      ready,
      checks,
      sdk: exposeSdkCatalog
        ? {
            connected: sdkConnected,
            functions: deps.sdkBridge.getFunctions().map((fn) => fn.name),
            states: deps.sdkBridge.getStates().map((state) => state.id),
            policies: deps.sdkBridge.getPolicies().map((policy) => policy.id),
            flows: deps.sdkBridge.getFlows().map((flow) => flow.id),
          }
        : { connected: sdkConnected },
      memory: {
        enabled: deps.config.memory.enabled,
        vectorIndex: deps.database.vectorEnabled,
        vectorDimensions: deps.database.vectorDimensions,
        vectorIndexWarning: deps.config.memory.enabled && !deps.database.vectorEnabled
          ? 'sqlite-vec unavailable — recall using brute-force fallback'
          : undefined,
      },
      channels: {
        web: deps.config.channels.web.enabled,
        whatsapp: deps.config.channels.whatsapp.enabled,
      },
      llm: deps.config.llm.provider,
      sunjet: exposeSdkCatalog
        ? {
            enabled: deps.config.sunjet.enabled,
            url: deps.config.sunjet.url,
            dualWriteSqlite: deps.config.sunjet.dual_write_sqlite,
            messageBackend: deps.messageStore ? 'sunjet' : 'sqlite',
          }
        : { enabled: deps.config.sunjet.enabled },
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
          migrations: resolveMigrationsFolder(),
          public: resolvePublicDir(),
          database: deps?.config.storage.database_path,
        },
        config: deps
          ? {
              name: deps.config.name,
              llm: deps.config.llm.provider,
              safetyMode: deps.config.safety.default_mode,
              memoryEnabled: deps.config.memory.enabled,
              vectorIndex: deps.database.vectorEnabled,
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
