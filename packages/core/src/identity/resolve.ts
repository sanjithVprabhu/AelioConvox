import type { Channel } from '@aelio/protocol';

export type IdentityConfig = {
  mappingFunction: 'phone' | 'email';
  allowAnonymous: boolean;
};

export function normalizePhone(phone: string): string {
  return phone.replace(/\D/g, '');
}

export function normalizeEmail(email: string): string {
  return email.trim().toLowerCase();
}

export function formatWhatsAppAddress(phone: string): string {
  const normalized = normalizePhone(phone);
  return normalized.startsWith('+') ? normalized : `+${normalized}`;
}

export function resolveCustomerExternalId(
  channel: Channel,
  channelAddress: string,
  identity: IdentityConfig,
): string {
  if (channel === 'whatsapp' && identity.mappingFunction === 'phone') {
    return normalizePhone(channelAddress);
  }

  if (channel === 'web' && identity.mappingFunction === 'email' && channelAddress.includes('@')) {
    return normalizeEmail(channelAddress);
  }

  return channelAddress;
}

export function resolveWhatsAppIdentity(from: string): {
  externalId: string;
  channelAddress: string;
} {
  const externalId = normalizePhone(from);
  return {
    externalId,
    channelAddress: formatWhatsAppAddress(from),
  };
}