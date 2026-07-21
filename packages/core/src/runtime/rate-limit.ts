import type { ConvoxMessageStore } from '../storage/messages.js';

export type RateLimitConfig = {
  perCustomerPerMinute: number;
  perCustomerPerDay: number;
};

/**
 * Rate limits are counted from Sunjet message rows (VSS store), not SQLite.
 */
export async function assertWithinRateLimit(
  messageStore: ConvoxMessageStore | undefined,
  customerId: string,
  config: RateLimitConfig,
): Promise<void> {
  if (!messageStore) {
    return;
  }

  const now = Date.now();
  const minuteCount = await messageStore.countUserMessages(customerId, now - 60_000);
  if (minuteCount >= config.perCustomerPerMinute) {
    throw new Error('Rate limit exceeded for this customer in the last minute');
  }

  const dayCount = await messageStore.countUserMessages(customerId, now - 86_400_000);
  if (dayCount >= config.perCustomerPerDay) {
    throw new Error('Rate limit exceeded for this customer today');
  }
}
