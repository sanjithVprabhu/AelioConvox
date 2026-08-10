import type { ApiValue, SunjetClient } from '@aelio/sunjet-client';

export type ConversationTelemetryEvent = {
  rowId: number;
  eventId: string;
  messageId: string;
  sessionId: string;
  customerId: string;
  customerExternalId: string;
  channel: string;
  channelAddress: string;
  role: string;
  content: string;
  createdAt: number;
  lifecycleState?: string;
  lifecycleStateReason?: string;
  intentLabel?: string;
  intentSummary?: string;
  intentStack?: Array<{
    id: string;
    label: string;
    summary: string;
    startedAt: number;
    lastActiveAt: number;
    expiresAt: number;
  }>;
  flowId?: string;
  flowStepId?: string;
  flowStepIndex?: number;
  flowStepGoal?: string;
  activePolicies?: Array<{ id: string; severity: string }>;
  toolsExecuted?: string[];
  pendingConfirmation: boolean;
};

function readUtf8(values: Record<string, ApiValue>, key: string): string {
  const entry = values[key];
  if (!entry || entry.type !== 'utf8') {
    return '';
  }
  return entry.value;
}

function readI64(values: Record<string, ApiValue>, key: string): number | undefined {
  const entry = values[key];
  if (!entry || entry.type !== 'i64') {
    return undefined;
  }
  return entry.value;
}

function readBool(values: Record<string, ApiValue>, key: string): boolean {
  const entry = values[key];
  return entry?.type === 'bool' ? entry.value : false;
}

function parseJsonArray<T>(raw: string): T[] | undefined {
  if (!raw) {
    return undefined;
  }
  try {
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) ? (parsed as T[]) : undefined;
  } catch {
    return undefined;
  }
}

function mapRow(rowId: number, values: Record<string, ApiValue>): ConversationTelemetryEvent {
  const intentStackRaw = readUtf8(values, 'intent_stack');
  const policiesRaw = readUtf8(values, 'active_policies');
  const toolsRaw = readUtf8(values, 'tools_executed');

  return {
    rowId,
    eventId: readUtf8(values, 'event_id'),
    messageId: readUtf8(values, 'message_id'),
    sessionId: readUtf8(values, 'session_id'),
    customerId: readUtf8(values, 'customer_id'),
    customerExternalId: readUtf8(values, 'customer_external_id'),
    channel: readUtf8(values, 'channel'),
    channelAddress: readUtf8(values, 'channel_address'),
    role: readUtf8(values, 'role'),
    content: readUtf8(values, 'content'),
    createdAt: readI64(values, 'created_at') ?? 0,
    lifecycleState: readUtf8(values, 'lifecycle_state') || undefined,
    lifecycleStateReason: readUtf8(values, 'lifecycle_state_reason') || undefined,
    intentLabel: readUtf8(values, 'intent_label') || undefined,
    intentSummary: readUtf8(values, 'intent_summary') || undefined,
    intentStack: parseJsonArray(intentStackRaw),
    flowId: readUtf8(values, 'flow_id') || undefined,
    flowStepId: readUtf8(values, 'flow_step_id') || undefined,
    flowStepIndex: readI64(values, 'flow_step_index'),
    flowStepGoal: readUtf8(values, 'flow_step_goal') || undefined,
    activePolicies: parseJsonArray(policiesRaw),
    toolsExecuted: parseJsonArray<string>(toolsRaw),
    pendingConfirmation: readBool(values, 'pending_confirmation'),
  };
}

type RuntimeSnapshotHistoryEntry = {
  role: string;
  content: string;
  at: number;
};

type RuntimeSnapshotPayload = {
  history?: RuntimeSnapshotHistoryEntry[];
  lifecycleState?: string | null;
  lifecycleReason?: string | null;
  flowProgress?: Record<string, { currentStepIndex: number; completedSteps: string[] }>;
  lastChannel?: string;
};

/**
 * Conversation telemetry sourced from the event-sourced Aelio runtime (`aelio_runtime_snapshots`)
 * instead of the legacy `convox_conversations` table. Once Sunjet is enabled, the runtime always
 * takes the `processAelioRuntimeMessage` path (see server/src/routes/widget.ts), which never
 * writes `convox_conversations` — that table is only populated by the older `processTurn` path.
 * This reads the subject snapshot's embedded history instead, which the runtime does maintain.
 */
export async function listRuntimeConversationTelemetry(
  client: SunjetClient,
  snapshotsTable: string,
  input: {
    limit?: number;
    since?: number;
    sessionId?: string;
    customerExternalId?: string;
  } = {},
): Promise<ConversationTelemetryEvent[]> {
  const limit = Math.min(Math.max(input.limit ?? 100, 1), 500);
  const subjectId = input.customerExternalId || input.sessionId;
  const filters: Array<{ col: string; op: 'eq'; value: ApiValue }> = [
    { col: 'snapshot_key', op: 'eq', value: { type: 'utf8', value: 'subject' } },
  ];
  if (subjectId) {
    filters.push({ col: 'subject_id', op: 'eq', value: { type: 'utf8', value: subjectId } });
  }

  const scan = await client.scanRows(snapshotsTable, {
    k: 500,
    filters,
  });

  const events: ConversationTelemetryEvent[] = [];
  for (const row of scan.rows ?? []) {
    const subject = readUtf8(row.values, 'subject_id');
    const payloadRaw = readUtf8(row.values, 'payload_json');
    let payload: RuntimeSnapshotPayload;
    try {
      payload = JSON.parse(payloadRaw) as RuntimeSnapshotPayload;
    } catch {
      continue;
    }
    for (const entry of payload.history ?? []) {
      if (input.since !== undefined && entry.at < input.since) {
        continue;
      }
      events.push({
        rowId: row.row_id,
        eventId: `${subject}:${entry.at}`,
        messageId: `${subject}:${entry.at}`,
        sessionId: subject,
        customerId: subject,
        customerExternalId: subject,
        channel: payload.lastChannel ?? '',
        channelAddress: '',
        role: entry.role,
        content: entry.content,
        createdAt: entry.at,
        lifecycleState: payload.lifecycleState ?? undefined,
        lifecycleStateReason: payload.lifecycleReason ?? undefined,
        pendingConfirmation: false,
      });
    }
  }

  return events.sort((a, b) => a.createdAt - b.createdAt).slice(-limit);
}

export async function listConversationTelemetry(
  client: SunjetClient,
  table: string,
  input: {
    limit?: number;
    since?: number;
    sessionId?: string;
    customerExternalId?: string;
  } = {},
): Promise<ConversationTelemetryEvent[]> {
  const limit = Math.min(Math.max(input.limit ?? 100, 1), 500);
  const filters: Array<{
    col: string;
    op: 'eq' | 'ge';
    value: ApiValue;
  }> = [];

  if (input.sessionId) {
    filters.push({
      col: 'session_id',
      op: 'eq',
      value: { type: 'utf8', value: input.sessionId },
    });
  }
  if (input.customerExternalId) {
    filters.push({
      col: 'customer_external_id',
      op: 'eq',
      value: { type: 'utf8', value: input.customerExternalId },
    });
  }
  if (input.since !== undefined) {
    filters.push({
      col: 'created_at',
      op: 'ge',
      value: { type: 'i64', value: input.since },
    });
  }

  const scan = await client.scanRows(table, {
    k: Math.max(limit * 4, 200),
    filters,
  });

  const events = (scan.rows ?? [])
    .map((row) => mapRow(row.row_id, row.values))
    .sort((a, b) => a.createdAt - b.createdAt)
    .slice(-limit);

  return events;
}