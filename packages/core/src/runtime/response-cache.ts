import type { ConvoxResponseCacheStore } from '../storage/cache.js';

export type ResponseCacheConfig = {
  enabled: boolean;
  similarityThreshold: number;
  ttlMinutes: number;
};

/**
 * Look for a cached reply to a near-identical earlier question from the SAME
 * customer that has not expired. Returns the reply on a hit (and bumps its hit
 * count), null otherwise. Only no-tool replies are ever stored (see storeCachedResponse).
 */
export async function lookupCachedResponse(input: {
  customerId: string;
  message: string;
  threshold: number;
  responseCacheStore: ConvoxResponseCacheStore;
}): Promise<string | null> {
  if (!input.responseCacheStore) {
    throw new Error('AelioDb responseCacheStore is required');
  }
  return input.responseCacheStore.lookup(input.customerId, input.message, input.threshold);
}

/** Cache a no-tool reply for a customer with a TTL. */
export async function storeCachedResponse(input: {
  customerId: string;
  message: string;
  reply: string;
  ttlMinutes: number;
  responseCacheStore: ConvoxResponseCacheStore;
}): Promise<void> {
  if (!input.responseCacheStore) {
    throw new Error('AelioDb responseCacheStore is required');
  }
  await input.responseCacheStore.store(input.customerId, input.message, input.reply, input.ttlMinutes);
}
