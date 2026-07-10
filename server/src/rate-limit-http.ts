/**
 * Simple in-memory sliding-window rate limiter for HTTP endpoints
 * (magic-link issuance). Not durable across restarts — sufficient for abuse
 * throttling; pair with SDK secret auth for production.
 */
type Bucket = { timestamps: number[] };

const buckets = new Map<string, Bucket>();

export function checkRateLimit(
  key: string,
  limit: number,
  windowMs: number,
): { allowed: boolean; retryAfterMs?: number } {
  const now = Date.now();
  const bucket = buckets.get(key) ?? { timestamps: [] };
  bucket.timestamps = bucket.timestamps.filter((ts) => now - ts < windowMs);

  if (bucket.timestamps.length >= limit) {
    const oldest = bucket.timestamps[0] ?? now;
    buckets.set(key, bucket);
    return { allowed: false, retryAfterMs: windowMs - (now - oldest) };
  }

  bucket.timestamps.push(now);
  buckets.set(key, bucket);
  return { allowed: true };
}

/** Test helper — clear all buckets. */
export function resetRateLimits(): void {
  buckets.clear();
}
