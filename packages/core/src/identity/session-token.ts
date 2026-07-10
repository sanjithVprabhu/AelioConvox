import { createHmac, timingSafeEqual } from 'node:crypto';

export type SessionTokenClaims = {
  customerId: string;
  externalId: string;
  email: string;
  exp: number;
};

function base64UrlEncode(value: string): string {
  return Buffer.from(value, 'utf8').toString('base64url');
}

function base64UrlDecode(value: string): string {
  return Buffer.from(value, 'base64url').toString('utf8');
}

function sign(secret: string, payloadB64: string): string {
  return createHmac('sha256', secret).update(payloadB64).digest('base64url');
}

/**
 * Issue an HMAC-signed session token after magic-link verification. The widget
 * must present this on `init` so `customerId` cannot be spoofed (SEC-001).
 */
export function createSessionToken(
  secret: string,
  claims: { customerId: string; externalId: string; email: string },
  ttlMinutes = 60,
): string {
  const exp = Math.floor(Date.now() / 1000) + ttlMinutes * 60;
  const payload = base64UrlEncode(
    JSON.stringify({
      customerId: claims.customerId,
      externalId: claims.externalId,
      email: claims.email,
      exp,
    }),
  );
  return `${payload}.${sign(secret, payload)}`;
}

/** Verify a session token and return claims, or null when invalid/expired. */
export function verifySessionToken(secret: string, token: string): SessionTokenClaims | null {
  const parts = token.split('.');
  if (parts.length !== 2) {
    return null;
  }
  const [payloadB64, signature] = parts;
  if (!payloadB64 || !signature) {
    return null;
  }

  const expected = sign(secret, payloadB64);
  const a = Buffer.from(signature);
  const b = Buffer.from(expected);
  if (a.length !== b.length || !timingSafeEqual(a, b)) {
    return null;
  }

  try {
    const parsed = JSON.parse(base64UrlDecode(payloadB64)) as SessionTokenClaims;
    if (
      !parsed.customerId ||
      !parsed.externalId ||
      !parsed.email ||
      typeof parsed.exp !== 'number'
    ) {
      return null;
    }
    if (parsed.exp < Math.floor(Date.now() / 1000)) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}
