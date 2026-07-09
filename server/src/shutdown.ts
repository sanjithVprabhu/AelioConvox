import type { FastifyInstance } from 'fastify';

const SHUTDOWN_TIMEOUT_MS = 10_000;

export function registerGracefulShutdown(app: FastifyInstance): void {
  let shuttingDown = false;

  const shutdown = async (signal: string) => {
    if (shuttingDown) {
      return;
    }
    shuttingDown = true;
    app.log.info({ signal }, 'Received shutdown signal');

    const forceExit = setTimeout(() => {
      app.log.error('Shutdown timed out — forcing exit');
      process.exit(1);
    }, SHUTDOWN_TIMEOUT_MS);
    forceExit.unref();

    try {
      await app.close();
      app.log.info('Aelio server shut down cleanly');
      process.exit(0);
    } catch (error) {
      app.log.error(error, 'Error during shutdown');
      process.exit(1);
    }
  };

  process.on('SIGTERM', () => void shutdown('SIGTERM'));
  process.on('SIGINT', () => void shutdown('SIGINT'));
}