import type { InboundMessage } from '../types.js';

type MetaWebhookPayload = {
  object?: string;
  entry?: Array<{
    changes?: Array<{
      value?: {
        metadata?: { phone_number_id?: string };
        messages?: Array<{
          id: string;
          from: string;
          timestamp: string;
          type: string;
          text?: { body: string };
          image?: { caption?: string };
          audio?: Record<string, unknown>;
          document?: { filename?: string; caption?: string };
          interactive?: {
            button_reply?: { title?: string };
            list_reply?: { title?: string; description?: string };
          };
        }>;
      };
    }>;
  }>;
};

export function parseWhatsAppWebhook(payload: unknown): InboundMessage[] {
  const data = payload as MetaWebhookPayload;
  if (data.object !== 'whatsapp_business_account') {
    return [];
  }

  const inbound: InboundMessage[] = [];
  for (const entry of data.entry ?? []) {
    for (const change of entry.changes ?? []) {
      const value = change.value;
      const phoneNumberId = value?.metadata?.phone_number_id;
      for (const message of value?.messages ?? []) {
        let text = '';
        let type: InboundMessage['type'] = 'unknown';

        if (message.type === 'text' && message.text?.body) {
          text = message.text.body;
          type = 'text';
        } else if (message.type === 'image') {
          text = message.image?.caption?.trim() || '[Image message received]';
          type = 'image';
        } else if (message.type === 'audio') {
          text = '[Audio message received]';
          type = 'audio';
        } else if (message.type === 'document') {
          text = message.document?.caption?.trim() || `[Document received${message.document?.filename ? `: ${message.document.filename}` : ''}]`;
          type = 'document';
        } else if (message.type === 'interactive') {
          text =
            message.interactive?.button_reply?.title?.trim() ||
            message.interactive?.list_reply?.title?.trim() ||
            message.interactive?.list_reply?.description?.trim() ||
            '[Interactive reply received]';
          type = 'interactive';
        }

        if (!text) {
          continue;
        }
        inbound.push({
          messageId: message.id,
          from: message.from,
          text,
          timestamp: Number(message.timestamp) * 1000,
          phoneNumberId,
          type,
        });
      }
    }
  }

  return inbound;
}
