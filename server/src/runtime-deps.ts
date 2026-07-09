import type { ConvoxMessageStore } from '@aelio/core';
import type { AelioDatabase } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import type { WhatsAppSender } from '@aelio/channels';
import type { SunjetClient } from '@aelio/sunjet-client';
import type { AelioConfig } from './config.js';
import type { ServerSdkBridge } from './sdk-bridge.js';

export type RuntimeDeps = {
  config: AelioConfig;
  database: AelioDatabase;
  llm: LLMProvider;
  sdkBridge: ServerSdkBridge;
  whatsappSender: WhatsAppSender | null;
  sunjetClient: SunjetClient | null;
  messageStore: ConvoxMessageStore | null;
};