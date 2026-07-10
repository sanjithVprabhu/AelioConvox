import { timingSafeEqual } from 'node:crypto';
import type { FastifyReply, FastifyRequest } from 'fastify';

/** Constant-time secret comparison — avoids leaking length/prefix via timing. */
export function secretsMatch(a: string | undefined, b: string | undefined): boolean {
  if (!a || !b) {
    return false;
  }
  const ba = Buffer.from(a);
  const bb = Buffer.from(b);
  if (ba.length !== bb.length) {
    return false;
  }
  return timingSafeEqual(ba, bb);
}

/** Extract a Bearer token from the Authorization header, if present. */
export function bearerToken(request: FastifyRequest): string | undefined {
  const header = request.headers.authorization;
  return header?.startsWith('Bearer ') ? header.slice('Bearer '.length) : undefined;
}

/**
 * Guard an HTTP route with the shared SDK secret (Authorization: Bearer <secret>).
 * Returns true when authorized; otherwise sends 401 and returns false — callers
 * do `if (!requireSecret(...)) return;`. Constant-time comparison throughout.
 */
export function requireSecret(
  request: FastifyRequest,
  reply: FastifyReply,
  secret: string,
): boolean {
  if (secretsMatch(bearerToken(request), secret)) {
    return true;
  }
  void reply.status(401).send({ error: 'Unauthorized' });
  return false;
}
