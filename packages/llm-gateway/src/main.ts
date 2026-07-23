// Entrypoint: boot the gateway HTTP server. Kept separate from server.ts (which only exports
// `createGatewayServer`) so importing the library never starts a listener.
import { createGatewayServer } from './server.js';

const port = Number(process.env.PORT ?? 8787);
const host = process.env.HOST ?? '0.0.0.0';

createGatewayServer().listen(port, host, () => {
  // eslint-disable-next-line no-console
  console.log(`[aelio-llm-gateway] listening on http://${host}:${port} (POST /v1/llm/complete)`);
});
