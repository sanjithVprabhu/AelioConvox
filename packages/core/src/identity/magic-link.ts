import { createHash, randomBytes } from 'node:crypto';
import type { ConvoxCustomerStore } from '../storage/customers.js';
import type { ConvoxMagicLinkStore } from '../storage/kv.js';
import { normalizeEmail } from './resolve.js';

export type MagicLinkResult = {
  token: string;
  url: string;
  email: string;
  expiresAt: Date;
};

export type VerifiedMagicLink = {
  customerId: string;
  externalId: string;
  email: string;
};

function hashToken(token: string): string {
  return createHash('sha256').update(token).digest('hex');
}

export async function createMagicLink(
  input: {
    email: string;
    externalId?: string;
    baseUrl: string;
    ttlMinutes?: number;
  },
  customerStore: ConvoxCustomerStore,
  magicLinkStore: ConvoxMagicLinkStore,
): Promise<MagicLinkResult> {
  if (!customerStore) {
    throw new Error('AelioDb customerStore is required');
  }
  if (!magicLinkStore) {
    throw new Error('AelioDb magicLinkStore is required');
  }

  const email = normalizeEmail(input.email);
  const externalId = input.externalId ?? email;
  const token = randomBytes(32).toString('hex');
  const tokenHash = hashToken(token);
  const now = new Date();
  const expiresAt = new Date(now.getTime() + (input.ttlMinutes ?? 15) * 60_000);

  // `ensureCustomer` atomically resolves-or-creates the customer AND records
  // the (already-verified) web channel address in one AelioDb round trip.
  const customerId = await customerStore.ensureCustomer(externalId, 'web', email);

  await magicLinkStore.create({
    tokenHash,
    email,
    customerId,
    externalId,
    expiresAtMs: expiresAt.getTime(),
  });

  const url = new URL('/auth/verify', input.baseUrl);
  url.searchParams.set('token', token);

  return { token, url: url.toString(), email, expiresAt };
}

export async function verifyMagicLink(
  token: string,
  magicLinkStore: ConvoxMagicLinkStore,
): Promise<VerifiedMagicLink | null> {
  if (!magicLinkStore) {
    throw new Error('AelioDb magicLinkStore is required');
  }

  const tokenHash = hashToken(token);
  const link = await magicLinkStore.findByTokenHash(tokenHash);
  if (!link || link.consumedAt || link.expiresAt < Date.now() || !link.customerId || !link.externalId) {
    return null;
  }
  // Atomic (best-effort) consume — two parallel verify requests cannot both
  // succeed (BUG-040).
  const consumed = await magicLinkStore.consume(link.id);
  if (!consumed) {
    return null;
  }
  return {
    customerId: link.customerId,
    externalId: link.externalId,
    email: link.email,
  };
}
