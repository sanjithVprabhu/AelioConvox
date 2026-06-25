import { index, integer, real, sqliteTable, text, uniqueIndex } from 'drizzle-orm/sqlite-core';

export const customers = sqliteTable(
  'customers',
  {
    id: text('id').primaryKey(),
    externalId: text('external_id'),
    displayName: text('display_name'),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    updatedAt: integer('updated_at', { mode: 'timestamp_ms' }).notNull(),
    metadata: text('metadata', { mode: 'json' }).$type<Record<string, unknown>>(),
  },
  (table) => [uniqueIndex('idx_customers_external').on(table.externalId)],
);

export const channelAddresses = sqliteTable(
  'channel_addresses',
  {
    id: text('id').primaryKey(),
    customerId: text('customer_id')
      .notNull()
      .references(() => customers.id),
    channel: text('channel').notNull(),
    address: text('address').notNull(),
    verifiedAt: integer('verified_at', { mode: 'timestamp_ms' }),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
  },
  (table) => [uniqueIndex('idx_chan_addr').on(table.channel, table.address)],
);

export const sessions = sqliteTable(
  'sessions',
  {
    id: text('id').primaryKey(),
    customerId: text('customer_id')
      .notNull()
      .references(() => customers.id),
    channel: text('channel').notNull(),
    status: text('status').notNull(),
    startedAt: integer('started_at', { mode: 'timestamp_ms' }).notNull(),
    lastActivityAt: integer('last_activity_at', { mode: 'timestamp_ms' }).notNull(),
    closedAt: integer('closed_at', { mode: 'timestamp_ms' }),
    summary: text('summary'),
    metadata: text('metadata', { mode: 'json' }).$type<Record<string, unknown>>(),
  },
  (table) => [
    index('idx_sessions_customer').on(table.customerId, table.lastActivityAt),
    index('idx_sessions_active').on(table.status, table.lastActivityAt),
  ],
);

export const messages = sqliteTable(
  'messages',
  {
    id: text('id').primaryKey(),
    sessionId: text('session_id')
      .notNull()
      .references(() => sessions.id),
    customerId: text('customer_id')
      .notNull()
      .references(() => customers.id),
    role: text('role').notNull(),
    content: text('content'),
    toolCall: text('tool_call', { mode: 'json' }).$type<Record<string, unknown>>(),
    toolResult: text('tool_result', { mode: 'json' }).$type<Record<string, unknown>>(),
    channel: text('channel').notNull(),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    tokensIn: integer('tokens_in'),
    tokensOut: integer('tokens_out'),
  },
  (table) => [index('idx_messages_session').on(table.sessionId, table.createdAt)],
);

export const memory = sqliteTable('memory', {
  id: text('id').primaryKey(),
  customerId: text('customer_id')
    .notNull()
    .references(() => customers.id),
  content: text('content').notNull(),
  embedding: text('embedding', { mode: 'json' }).$type<number[]>(),
  sourceSessionId: text('source_session_id').references(() => sessions.id),
  createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
  expiresAt: integer('expires_at', { mode: 'timestamp_ms' }),
  confidence: real('confidence').default(1.0),
  category: text('category'),
});

export const functionCalls = sqliteTable(
  'function_calls',
  {
    id: text('id').primaryKey(),
    sessionId: text('session_id')
      .notNull()
      .references(() => sessions.id),
    customerId: text('customer_id')
      .notNull()
      .references(() => customers.id),
    functionName: text('function_name').notNull(),
    args: text('args', { mode: 'json' }).$type<Record<string, unknown>>(),
    result: text('result', { mode: 'json' }).$type<Record<string, unknown>>(),
    status: text('status').notNull(),
    safetyLevel: text('safety_level').notNull(),
    requiredConfirmation: integer('required_confirmation', { mode: 'boolean' }).default(false),
    confirmed: integer('confirmed', { mode: 'boolean' }),
    durationMs: integer('duration_ms'),
    errorMessage: text('error_message'),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
  },
  (table) => [
    index('idx_fc_session').on(table.sessionId, table.createdAt),
    index('idx_fc_customer').on(table.customerId, table.createdAt),
  ],
);

export const sdkConnections = sqliteTable('sdk_connections', {
  id: text('id').primaryKey(),
  connectionToken: text('connection_token').notNull(),
  sdkVersion: text('sdk_version'),
  language: text('language'),
  connectedAt: integer('connected_at', { mode: 'timestamp_ms' }).notNull(),
  lastHeartbeatAt: integer('last_heartbeat_at', { mode: 'timestamp_ms' }).notNull(),
  functions: text('functions', { mode: 'json' })
    .notNull()
    .$type<Array<Record<string, unknown>>>(),
});

export const magicLinks = sqliteTable('magic_links', {
  id: text('id').primaryKey(),
  tokenHash: text('token_hash').notNull(),
  email: text('email').notNull(),
  customerId: text('customer_id').references(() => customers.id),
  externalId: text('external_id'),
  expiresAt: integer('expires_at', { mode: 'timestamp_ms' }).notNull(),
  consumedAt: integer('consumed_at', { mode: 'timestamp_ms' }),
  createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
});

export const reflections = sqliteTable(
  'reflections',
  {
    id: text('id').primaryKey(),
    sessionId: text('session_id').notNull(),
    customerId: text('customer_id').notNull(),
    outcome: text('outcome').notNull(), // 'resolved' | 'unresolved' | 'unclear'
    score: real('score'),
    summary: text('summary'),
    issues: text('issues', { mode: 'json' }).$type<string[]>(),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
  },
  (table) => [
    index('idx_reflections_session').on(table.sessionId),
    index('idx_reflections_customer').on(table.customerId, table.createdAt),
  ],
);

export const responseCache = sqliteTable(
  'response_cache',
  {
    id: text('id').primaryKey(),
    customerId: text('customer_id').notNull(),
    query: text('query').notNull(),
    embedding: text('embedding', { mode: 'json' }).$type<number[]>(),
    reply: text('reply').notNull(),
    hits: integer('hits').default(0),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    expiresAt: integer('expires_at', { mode: 'timestamp_ms' }).notNull(),
  },
  (table) => [index('idx_respcache_customer').on(table.customerId, table.expiresAt)],
);

export const proactiveMessages = sqliteTable(
  'proactive_messages',
  {
    id: text('id').primaryKey(),
    customerId: text('customer_id').notNull(),
    channel: text('channel').notNull(),
    toAddress: text('to_address').notNull(),
    content: text('content').notNull(),
    dedupKey: text('dedup_key'),
    status: text('status').notNull(), // 'sent' | 'blocked'
    reason: text('reason'),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
  },
  (table) => [index('idx_proactive_customer').on(table.customerId, table.createdAt)],
);

export const jobQueue = sqliteTable(
  'job_queue',
  {
    id: text('id').primaryKey(),
    queue: text('queue').notNull(),
    payload: text('payload', { mode: 'json' }).notNull().$type<Record<string, unknown>>(),
    status: text('status').notNull(),
    attempts: integer('attempts').default(0),
    maxAttempts: integer('max_attempts').default(5),
    nextRunAt: integer('next_run_at', { mode: 'timestamp_ms' }).notNull(),
    lockedBy: text('locked_by'),
    lockedAt: integer('locked_at', { mode: 'timestamp_ms' }),
    createdAt: integer('created_at', { mode: 'timestamp_ms' }).notNull(),
    completedAt: integer('completed_at', { mode: 'timestamp_ms' }),
    errorMessage: text('error_message'),
  },
  (table) => [index('idx_jobs_pending').on(table.queue, table.status, table.nextRunAt)],
);

export const schema = {
  customers,
  channelAddresses,
  sessions,
  messages,
  memory,
  functionCalls,
  sdkConnections,
  magicLinks,
  jobQueue,
  reflections,
  proactiveMessages,
  responseCache,
};

export type DatabaseSchema = typeof schema;