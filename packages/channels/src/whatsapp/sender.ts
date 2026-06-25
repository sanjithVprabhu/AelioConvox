import type { OutboundContent, WhatsAppSender } from '../types.js';

export type MetaWhatsAppConfig = {
  phoneNumberId: string;
  accessToken: string;
};

export class MetaWhatsAppSender implements WhatsAppSender {
  constructor(private readonly config: MetaWhatsAppConfig) {}

  async send(to: string, content: OutboundContent): Promise<void> {
    const body =
      content.type === 'template'
        ? {
            messaging_product: 'whatsapp',
            to,
            type: 'template',
            template: {
              name: content.templateName,
              language: { code: content.languageCode ?? 'en_US' },
              components:
                content.bodyParameters && content.bodyParameters.length > 0
                  ? [
                      {
                        type: 'body',
                        parameters: content.bodyParameters.map((value) => ({
                          type: 'text',
                          text: value,
                        })),
                      },
                    ]
                  : undefined,
            },
          }
        : content.type === 'media'
          ? {
              messaging_product: 'whatsapp',
              to,
              type: content.mediaType,
              [content.mediaType]: {
                ...(content.mediaId ? { id: content.mediaId } : {}),
                ...(content.link ? { link: content.link } : {}),
                ...(content.caption ? { caption: content.caption } : {}),
                ...(content.filename ? { filename: content.filename } : {}),
              },
            }
          : content.type === 'receipt'
            ? {
                messaging_product: 'whatsapp',
                status: content.action === 'mark_read' ? 'read' : undefined,
                message_id: content.messageId,
                typing_indicator:
                  content.action === 'typing_on'
                    ? { type: 'text' }
                    : content.action === 'typing_off'
                      ? { type: 'none' }
                      : undefined,
              }
            : {
                messaging_product: 'whatsapp',
                to,
                type: 'text',
                text: { body: content.text, preview_url: content.previewUrl ?? false },
              };

    const response = await fetch(
      `https://graph.facebook.com/v21.0/${this.config.phoneNumberId}/messages`,
      {
        method: 'POST',
        headers: {
          authorization: `Bearer ${this.config.accessToken}`,
          'content-type': 'application/json',
        },
        body: JSON.stringify(body),
      },
    );

    if (!response.ok) {
      const body = await response.text();
      throw new Error(`WhatsApp send failed (${response.status}): ${body}`);
    }
  }
}

type SentMessage = {
  to: string;
  text: string;
  sentAt: number;
};

const sentMessages: SentMessage[] = [];

export class MockWhatsAppSender implements WhatsAppSender {
  async send(to: string, content: OutboundContent): Promise<void> {
    sentMessages.push({
      to,
      text:
        content.type === 'template'
          ? `[template:${content.templateName}] ${(content.bodyParameters ?? []).join(' ')}`
          : content.type === 'media'
            ? `[${content.mediaType}] ${content.caption ?? content.link ?? content.mediaId ?? ''}`.trim()
            : content.type === 'receipt'
              ? `[${content.action}] ${content.messageId}`
              : content.text,
      sentAt: Date.now(),
    });
  }
}

export function getMockWhatsAppMessages(): SentMessage[] {
  return [...sentMessages];
}

export function clearMockWhatsAppMessages(): void {
  sentMessages.length = 0;
}

export function getLastMockWhatsAppMessage(to?: string): SentMessage | undefined {
  const filtered = to ? sentMessages.filter((entry) => entry.to === to) : sentMessages;
  return filtered.at(-1);
}
