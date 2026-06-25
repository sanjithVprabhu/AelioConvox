import type { AelioDatabase } from '@aelio/db';
import { messages } from '@aelio/db';
import { and, count, eq, gte } from 'drizzle-orm';

export type RateLimitConfig = {
  perCustomerPerMinute: number;
  perCustomerPerDay: number;
};

export async function assertWithinRateLimit(
  db: AelioDatabase['db'],
  customerId: string,
  config: RateLimitConfig,
): Promise<void> {
  const now = Date.now();
  const minuteAgo = new Date(now - 60_000);
  const dayAgo = new Date(now - 86_400_000);

  const [minuteCount] = await db
    .select({ value: count() })
    .from(messages)
    .where(
      and(
        eq(messages.customerId, customerId),
        eq(messages.role, 'user'),
        gte(messages.createdAt, minuteAgo),
      ),
    );

  if ((minuteCount?.value ?? 0) >= config.perCustomerPerMinute) {
    throw new Error('Rate limit exceeded for this customer in the last minute');
  }

  const [dayCount] = await db
    .select({ value: count() })
    .from(messages)
    .where(
      and(
        eq(messages.customerId, customerId),
        eq(messages.role, 'user'),
        gte(messages.createdAt, dayAgo),
      ),
    );

  if ((dayCount?.value ?? 0) >= config.perCustomerPerDay) {
    throw new Error('Rate limit exceeded for this customer today');
  }
}
