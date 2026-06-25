export type InboundMessage = {
  messageId: string;
  from: string;
  text: string;
  timestamp: number;
  phoneNumberId?: string;
  type?: 'text' | 'image' | 'audio' | 'document' | 'interactive' | 'unknown';
};

export type OutboundContent =
  | {
      type?: 'text';
      text: string;
      previewUrl?: boolean;
    }
  | {
      type: 'template';
      templateName: string;
      languageCode?: string;
      bodyParameters?: string[];
    }
  | {
      type: 'media';
      mediaType: 'image' | 'audio' | 'document';
      mediaId?: string;
      link?: string;
      caption?: string;
      filename?: string;
    }
  | {
      type: 'receipt';
      messageId: string;
      action: 'mark_read' | 'typing_on' | 'typing_off';
    };

export interface WhatsAppSender {
  send(to: string, content: OutboundContent): Promise<void>;
}
