import type { AelioDatabase } from '@aelio/db';
import { channelAddresses, customers, magicLinks } from '@aelio/db';
import { and, eq } from 'drizzle-orm';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
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
  database: AelioDatabase,
  input: {
    email: string;
    externalId?: string;
    baseUrl: string;
    ttlMinutes?: number;
  },
): Promise<MagicLinkResult> {
  const email = normalizeEmail(input.email);
  const externalId = input.externalId ?? email;
  const token = randomBytes(32).toString('hex');
  const tokenHash = hashToken(token);
  const now = new Date();
  const expiresAt = new Date(now.getTime() + (input.ttlMinutes ?? 15) * 60_000);

  const existingCustomer = await database.db
    .select()
    .from(customers)
    .where(eq(customers.externalId, externalId))
    .limit(1);

  const customerId = existingCustomer[0]?.id ?? randomUUID();

  if (!existingCustomer[0]) {
    await database.db.insert(customers).values({
      id: customerId,
      externalId,
      displayName: email,
      createdAt: now,
      updatedAt: now,
    });
  }

  const existingAddress = await database.db
    .select()
    .from(channelAddresses)
    .where(and(eq(channelAddresses.channel, 'web'), eq(channelAddresses.address, email)))
    .limit(1);

  if (!existingAddress[0]) {
    await database.db.insert(channelAddresses).values({
      id: randomUUID(),
      customerId,
      channel: 'web',
      address: email,
      createdAt: now,
    });
  }

  await database.db.insert(magicLinks).values({
    id: randomUUID(),
    tokenHash,
    email,
    customerId,
    externalId,
    expiresAt,
    createdAt: now,
  });

  const url = new URL('/auth/verify', input.baseUrl);
  url.searchParams.set('token', token);

  return { token, url: url.toString(), email, expiresAt };
}

export async function verifyMagicLink(
  database: AelioDatabase,
  token: string,
): Promise<VerifiedMagicLink | null> {
  const tokenHash = hashToken(token);
  const now = new Date();

  const consumed = database.sqlite
    .prepare(
      `UPDATE magic_links
       SET consumed_at = ?
       WHERE token_hash = ? AND consumed_at IS NULL AND expires_at >= ?
       RETURNING id, email, customer_id, external_id`,
    )
    .get(now.getTime(), tokenHash, now.getTime()) as
    | { id: string; email: string; customer_id: string; external_id: string }
    | undefined;

  if (!consumed?.customer_id || !consumed.external_id) {
    return null;
  }

  await database.db
    .update(channelAddresses)
    .set({ verifiedAt: now })
    .where(and(eq(channelAddresses.channel, 'web'), eq(channelAddresses.address, consumed.email)));

  return {
    customerId: consumed.customer_id,
    externalId: consumed.external_id,
    email: consumed.email,
  };
}