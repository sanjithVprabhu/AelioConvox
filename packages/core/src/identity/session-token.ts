import { createHmac, timingSafeEqual } from 'node:crypto';

export type WidgetSessionClaims = {
  customerId: string;
  externalId: string;
  email: string;
  exp: number;
};

function b64urlEncode(value: string | Buffer): string {
  return Buffer.from(value)
    .toString('base64')
    .replace(/\+/g, '-')
    .replace(/\//g, '_')
    .replace(/=+$/g, '');
}

function b64urlDecode(value: string): Buffer {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/');
  const pad = padded.length % 4 === 0 ? '' : '='.repeat(4 - (padded.length % 4));
  return Buffer.from(padded + pad, 'base64');
}

function sign(payloadB64: string, secret: string): string {
  return b64urlEncode(createHmac('sha256', secret).update(payloadB64).digest());
}

/**
 * Issue a short-lived widget session token after magic-link verification.
 * Bound to verified identity — widget init must present this when anonymous
 * access is disabled.
 */
export function issueWidgetSessionToken(
  secret: string,
  claims: Omit<WidgetSessionClaims, 'exp'>,
  ttlMinutes = 60,
): string {
  const payload: WidgetSessionClaims = {
    ...claims,
    exp: Date.now() + ttlMinutes * 60_000,
  };
  const payloadB64 = b64urlEncode(JSON.stringify(payload));
  return `${payloadB64}.${sign(payloadB64, secret)}`;
}

export function verifyWidgetSessionToken(
  secret: string,
  token: string,
): WidgetSessionClaims | null {
  const [payloadB64, signature] = token.split('.');
  if (!payloadB64 || !signature) {
    return null;
  }

  const expected = sign(payloadB64, secret);
  const providedBuf = Buffer.from(signature);
  const expectedBuf = Buffer.from(expected);
  if (
    providedBuf.length !== expectedBuf.length ||
    !timingSafeEqual(providedBuf, expectedBuf)
  ) {
    return null;
  }

  try {
    const claims = JSON.parse(b64urlDecode(payloadB64).toString('utf8')) as WidgetSessionClaims;
    if (
      typeof claims.customerId !== 'string' ||
      typeof claims.externalId !== 'string' ||
      typeof claims.email !== 'string' ||
      typeof claims.exp !== 'number' ||
      claims.exp < Date.now()
    ) {
      return null;
    }
    return claims;
  } catch {
    return null;
  }
}
