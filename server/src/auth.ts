import { timingSafeEqual } from 'node:crypto';
import type { FastifyRequest } from 'fastify';

function safeEqual(a: string, b: string): boolean {
  const aBuf = Buffer.from(a);
  const bBuf = Buffer.from(b);
  if (aBuf.length !== bBuf.length) {
    return false;
  }
  return timingSafeEqual(aBuf, bBuf);
}

/** Shared Bearer / x-aelio-secret check used by proactive, telemetry, and magic-link routes. */
export function authorized(request: FastifyRequest, secret: string): boolean {
  const header = request.headers.authorization;
  const bearer = header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
  const provided = bearer ?? (request.headers['x-aelio-secret'] as string | undefined);
  if (!provided) {
    return false;
  }
  return safeEqual(provided, secret);
}
