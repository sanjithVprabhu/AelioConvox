import type { AelioDatabase } from '@aelio/db';
import type { LLMProvider } from '@aelio/llm';
import type { WhatsAppSender } from '@aelio/channels';
import type { AelioConfig } from './config.js';
import type { ServerSdkBridge } from './sdk-bridge.js';

export type RuntimeDeps = {
  config: AelioConfig;
  database: AelioDatabase;
  llm: LLMProvider;
  sdkBridge: ServerSdkBridge;
  whatsappSender: WhatsAppSender | null;
};