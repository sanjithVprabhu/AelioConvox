/** Default HTTP/WebSocket port for the Aelio server (override with AELIO_PORT). */
export const DEFAULT_AELIO_PORT = 3010;

/** Resolve Aelio server port from env (AELIO_PORT, legacy AELIO_SERVER_PORT). */
export function resolveAelioPort(fallback = DEFAULT_AELIO_PORT) {
  const raw = process.env.AELIO_PORT ?? process.env.AELIO_SERVER_PORT;
  if (raw !== undefined && raw !== '') {
    const port = Number(raw);
    if (Number.isInteger(port) && port > 0) return port;
  }
  return fallback;
}

export function aelioWsUrl() {
  return process.env.AELIO_SERVER_URL ?? `ws://127.0.0.1:${resolveAelioPort()}`;
}

export function aelioHttpUrl() {
  const fromEnv = process.env.AELIO_SERVER_URL;
  if (fromEnv) {
    return fromEnv.replace(/^ws/i, 'http');
  }
  return `http://127.0.0.1:${resolveAelioPort()}`;
}
