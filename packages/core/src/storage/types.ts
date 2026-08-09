import type { AelioDbClient } from '@aelio/db-client';

export type AelioDbTableNames = {
  messages: string;
  conversations: string;
  memories: string;
  compactions: string;
  runtimeState: string;
  // Harness tables
  harnessTools: string;
  harnessCapabilities: string;
  harnessBindings: string;
  harnessSuspensions: string;
  harnessLedger: string;
  harnessTraces: string;
  // Identity / session / lifecycle tables
  customers: string;
  channelAddresses: string;
  sessions: string;
  jobQueue: string;
  responseCache: string;
  functionCalls: string;
  turnApiCalls: string;
  reflections: string;
  proactiveMessages: string;
  inboundDedup: string;
  magicLinks: string;
  sdkConnections: string;
  sdkCatalog: string;
  archetypes: string;
  aspects: string;
  axisNodes: string;
};

export type AelioDbStorageConfig = {
  client: AelioDbClient;
  tables: AelioDbTableNames;
  embedDim: number;
};

import type { ConversationTurnContext } from './context.js';

export type MessageStoreAppendInput = {
  sessionId: string;
  customerId: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  channel: string;
  toolCall?: Record<string, unknown>;
  toolResult?: Record<string, unknown>;
  context?: ConversationTurnContext;
};

export type MessageStoreHistoryRow = {
  role: 'user' | 'assistant' | 'system';
  content: string;
};