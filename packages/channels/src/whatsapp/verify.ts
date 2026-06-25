import { createHmac, timingSafeEqual } from 'node:crypto';

export function verifyWhatsAppSignature(
  rawBody: string,
  signatureHeader: string | undefined,
  appSecret: string,
): boolean {
  if (!signatureHeader?.startsWith('sha256=')) {
    return false;
  }

  const expected = createHmac('sha256', appSecret).update(rawBody).digest('hex');
  const provided = signatureHeader.slice('sha256='.length);

  try {
    return timingSafeEqual(Buffer.from(expected), Buffer.from(provided));
  } catch {
    return false;
  }
}