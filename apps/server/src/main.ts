import { loadConfig } from './config.js';
import { createApp } from './app.js';
import { registerGracefulShutdown } from './shutdown.js';

const config = loadConfig();
const { app } = await createApp(config);
const { host, port } = config.server;

registerGracefulShutdown(app);

try {
  await app.listen({ host, port });
  app.log.info(
    {
      name: config.name,
      host,
      port,
      database: config.storage.database_path,
      llm: config.llm.provider,
      widget: `http://${host === '0.0.0.0' ? 'localhost' : host}:${port}/widget.js`,
      demo: `http://${host === '0.0.0.0' ? 'localhost' : host}:${port}/demo.html`,
    },
    'Aelio server started',
  );
} catch (error) {
  app.log.error(error);
  process.exit(1);
}