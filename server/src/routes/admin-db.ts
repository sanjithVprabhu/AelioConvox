import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify';
import { listTurnApiCalls } from '@aelio/core/edge';
import { AelioDbHttpError } from '@aelio/db-client';
import type {
  ApiValue,
  QueryRequest,
  RowValues,
  ScanResponse,
  SchemaResponse,
} from '@aelio/db-client';
import { resolvePublicDir } from '../paths.js';
import { requireSecret } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

type TableStatus = 'active' | 'mirror' | 'schema-only';

type TableCatalogEntry = {
  /** Stable config key. */
  key: string;
  /** Actual table name in Aelio database (configurable). */
  name: string;
  label: string;
  description: string;
  status: TableStatus;
  writtenBy: string;
};

/**
 * Human-facing metadata for each logical Aelio DB table. Keys map to
 * `config.aelioDb.tables.*`; the real table name is resolved at request time so
 * customised table names still work. `status` reflects what actually writes to
 * the table today (audit of the Aelio codebase).
 */
const TABLE_META: Array<{
  key: keyof RuntimeDeps['config']['aelioDb']['tables'];
  label: string;
  description: string;
  status: TableStatus;
  writtenBy: string;
}> = [
  {
    key: 'messages',
    label: 'Messages',
    description:
      'Raw conversation messages (L0). BM25 + embedding. Primary store: Aelio DB (not SQLite).',
    status: 'active',
    writtenBy: 'ConvoxMessageStore.appendMessage',
  },
  {
    key: 'conversations',
    label: 'Conversations',
    description:
      'Per-turn telemetry (intent, flows, tools). Written to Aelio database — powers the telemetry dashboard.',
    status: 'active',
    writtenBy: 'appendConversationRecord',
  },
  {
    key: 'memories',
    label: 'Memories',
    description:
      'Long-term customer memories (profile, preferences, insights). Written and recalled via Aelio database vector search.',
    status: 'active',
    writtenBy: 'ConvoxMemoryStore (extractMemories / reflectOnSession)',
  },
  {
    key: 'compactions',
    label: 'Immediate Context',
    description:
      'Immediate Context Engine buckets: short-term conversation context per customer, tiered by age (5–15m, 15–30m, 30–60m, 1–24h). Buckets slide down tiers as they age and are condensed on merge.',
    status: 'active',
    writtenBy: 'ImmediateContextEngine (roll pass during turns)',
  },
  {
    key: 'runtime_state',
    label: 'Runtime State',
    description: 'Ephemeral scoped state with TTL. Swept by aelioDb-daemon.',
    status: 'schema-only',
    writtenBy: '(planned / daemon sweep)',
  },
  {
    key: 'harness_tools',
    label: 'Harness Tools',
    description: 'Lighthouse tool mirror — one row per registered SDK tool, re-synced per registry hash.',
    status: 'active',
    writtenBy: 'LighthouseMirror.sync',
  },
  {
    key: 'harness_capabilities',
    label: 'Harness Capabilities',
    description: 'Capability taxonomy nodes for planner grounding + feasibility probes.',
    status: 'active',
    writtenBy: 'LighthouseMirror.sync',
  },
  {
    key: 'harness_bindings',
    label: 'Harness Bindings',
    description: 'Instruction→tool binding cache. Written by bindInstructions when AelioDb is enabled.',
    status: 'active',
    writtenBy: 'bindInstructions (binding cache)',
  },
  {
    key: 'harness_suspensions',
    label: 'Harness Suspensions',
    description:
      'Suspended plans awaiting user input. AelioDb is the source of truth when configured; SQLite is a fallback only for non-AelioDb deployments.',
    status: 'active',
    writtenBy: 'SuspensionStore',
  },
  {
    key: 'harness_ledger',
    label: 'Harness Ledger',
    description:
      'Per-instruction idempotency ledger. Written to AelioDb exclusively when configured (turn-scoped scan/insert).',
    status: 'active',
    writtenBy: 'loadLedgerForTurn / persistLedgerEntry',
  },
  {
    key: 'harness_traces',
    label: 'Harness Traces',
    description: 'Append-only harness trace firehose. Written to Aelio database.',
    status: 'active',
    writtenBy: 'HarnessTracer.trace',
  },
  {
    key: 'customers',
    label: 'Customers',
    description: 'Customer records (external id, display name, metadata). Primary store when AelioDb is configured.',
    status: 'active',
    writtenBy: 'ConvoxCustomerStore',
  },
  {
    key: 'channel_addresses',
    label: 'Channel Addresses',
    description: 'Customer channel addresses (phone/email/etc), written alongside customers.',
    status: 'active',
    writtenBy: 'ConvoxCustomerStore.ensureCustomer',
  },
  {
    key: 'sessions',
    label: 'Sessions',
    description: 'Conversation sessions per customer/channel, including pending-confirmation and summary metadata.',
    status: 'active',
    writtenBy: 'ConvoxSessionStore',
  },
  {
    key: 'job_queue',
    label: 'Job Queue',
    description: 'Inbound/outbound delivery job queue with claim/complete/fail/retry semantics.',
    status: 'active',
    writtenBy: 'ConvoxJobStore (enqueueJob / claimJob / completeJob / failJob)',
  },
  {
    key: 'response_cache',
    label: 'Response Cache',
    description: 'Semantic cache of no-tool replies, keyed by customer + embedding similarity.',
    status: 'active',
    writtenBy: 'ConvoxResponseCacheStore',
  },
  {
    key: 'function_calls',
    label: 'Function Calls',
    description: 'Audit log of every SDK function invocation (args, result, safety level, confirmation).',
    status: 'active',
    writtenBy: 'ConvoxFunctionCallStore (logFunctionCall)',
  },
  {
    key: 'turn_api_calls',
    label: 'Turn API Calls',
    description: 'Per-turn LLM/embedding call telemetry (prompt summary, tokens, duration).',
    status: 'active',
    writtenBy: 'recordTurnApiCall (TurnApiCallsAelioDbConfig)',
  },
  {
    key: 'reflections',
    label: 'Reflections',
    description: 'Daemon-generated session reflections (outcome, score, insight, follow-up).',
    status: 'active',
    writtenBy: 'ConvoxReflectionStore (reflectOnSession)',
  },
  {
    key: 'proactive_messages',
    label: 'Proactive Messages',
    description: 'Sent/blocked proactive message log (dedup + daily frequency cap enforcement).',
    status: 'active',
    writtenBy: 'ConvoxProactiveStore (sendProactiveMessage)',
  },
  {
    key: 'inbound_dedup',
    label: 'Inbound Dedup',
    description: 'Inbound webhook/ingest message id dedup, to drop webhook redeliveries.',
    status: 'active',
    writtenBy: 'ConvoxInboundDedupStore.claim',
  },
  {
    key: 'magic_links',
    label: 'Magic Links',
    description: 'Web magic-link auth tokens (hashed), single-use, TTL-bound.',
    status: 'active',
    writtenBy: 'ConvoxMagicLinkStore (createMagicLink / verifyMagicLink)',
  },
  {
    key: 'sdk_connections',
    label: 'SDK Connections',
    description: 'Connected Aelio SDK instances (function catalog, heartbeat) for multi-instance visibility.',
    status: 'active',
    writtenBy: 'ConvoxSdkConnectionStore (ServerSdkBridge)',
  },
  {
    key: 'archetypes',
    label: 'Archetypes',
    description:
      'Archetype valence engine: per-aspect positive/negative/neutral buckets (keyword + description + usage + inference + response guidance) embedded for stance detection. Matched by recomputed cosine (not RRF) against each incoming message. Linked to the mother collection via aspect_id.',
    status: 'active',
    writtenBy: 'ConvoxArchetypeStore (seeded at startup; grown by aspect discovery)',
  },
  {
    key: 'aspects',
    label: 'Aspects (mother)',
    description:
      'Self-learning stance registry: the "things" identified from conversation. Each aspect owns a pos/neu/neg bucket set. Grown post-turn by aspect discovery (staged as candidate until active).',
    status: 'active',
    writtenBy: 'ConvoxAspectStore (seeded builtins; discoverAspects per turn)',
  },
  {
    key: 'axis_nodes',
    label: 'Harness Axes',
    description:
      'User-specific Harness Axis graph: root nodes per (customer, aspect) and occurrence chains linked by previous/head edges. Traversed within a temporal scope to recall continuing concerns.',
    status: 'active',
    writtenBy: 'ConvoxAxisStore (recordOccurrence post-turn; traverseAxis on recall)',
  },
];

const MAX_SCAN_LIMIT = 2000;
const DEFAULT_SCAN_LIMIT = 100;
const VECTOR_PREVIEW = 6;

function buildCatalog(deps: RuntimeDeps): TableCatalogEntry[] {
  const tables = deps.config.aelioDb.tables;
  return TABLE_META.map((meta) => ({
    key: meta.key,
    name: tables[meta.key],
    label: meta.label,
    description: meta.description,
    status: meta.status,
    writtenBy: meta.writtenBy,
  }));
}

/** Resolve a table name from the request, rejecting anything not in the catalog. */
function resolveTable(deps: RuntimeDeps, requested: string): TableCatalogEntry | null {
  const catalog = buildCatalog(deps);
  return (
    catalog.find((entry) => entry.name === requested || entry.key === requested) ?? null
  );
}

/** 503 helper for when AelioDb is disabled or unreachable. */
function aelioDbUnavailable(deps: RuntimeDeps, reply: FastifyReply) {
  return reply.status(503).send({
    enabled: false,
    message: deps.config.aelioDb.enabled
      ? 'AelioDb is enabled but the client is not connected — check that Rust Aelio server is reachable.'
      : 'AelioDb is not enabled. Set aelioDb.enabled: true in config.yaml and restart to browse the Aelio database database.',
  });
}

/**
 * Shrink heavy column values (embeddings) for list views so a page of rows
 * does not ship megabytes of floats. Returns a shallow-cloned value map.
 */
function lightenRow(values: RowValues): RowValues {
  const out: RowValues = {};
  for (const [col, value] of Object.entries(values)) {
    if (value.type === 'vector' && Array.isArray(value.value) && value.value.length > VECTOR_PREVIEW) {
      out[col] = {
        type: 'vector',
        value: value.value.slice(0, VECTOR_PREVIEW),
      } as ApiValue;
      (out[col] as unknown as { __omitted: number }).__omitted = value.value.length;
    } else {
      out[col] = value;
    }
  }
  return out;
}

function numericValue(value: ApiValue | undefined): number | null {
  if (!value) return null;
  if (value.type === 'i64' || value.type === 'f64') return value.value;
  return null;
}

function stringValue(value: ApiValue | undefined): string {
  if (!value) return '';
  if (value.type === 'utf8') return String(value.value ?? '');
  return '';
}

function queryUtf8(value: string): ApiValue {
  return { type: 'utf8', value };
}

function parsePayload(value: string): unknown {
  if (!value) return null;
  try {
    return JSON.parse(value);
  } catch {
    return { raw: value };
  }
}

/** Translate a AelioDbHttpError into an equivalent HTTP reply. */
function forwardError(error: unknown, reply: FastifyReply) {
  if (error instanceof AelioDbHttpError) {
    return reply.status(error.status).send({ error: error.body || error.message });
  }
  const message = error instanceof Error ? error.message : String(error);
  return reply.status(502).send({ error: `AelioDb request failed: ${message}` });
}

export async function registerAdminDbRoutes(app: FastifyInstance, deps: RuntimeDeps) {
  const secret = deps.config.secret;

  // Static shell — safe to serve unauthenticated; every data call below is gated
  // behind the SDK secret (the browser cannot attach a bearer header on nav).
  app.get('/admin/db', async (_request, reply) => {
    const html = readFileSync(join(resolvePublicDir(), 'database.html'), 'utf8');
    return reply.type('text/html').send(html);
  });

  // --- Overview: catalog + connectivity ------------------------------------
  app.get('/api/v1/admin/db/tables', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }

    const catalog = buildCatalog(deps);
    const base = {
      dataset: deps.config.name,
      embedDim: deps.config.aelioDb.embed_dim,
      url: deps.config.aelioDb.url,
    };

    const client = deps.aelioDbClient;
    const tables = await Promise.all(
      catalog.map(async (entry) => {
        try {
          const schema: SchemaResponse = await client.getSchema(entry.name);
          return { ...entry, exists: true, columns: schema.columns };
        } catch (error) {
          if (error instanceof AelioDbHttpError && error.status === 404) {
            return { ...entry, exists: false, columns: [] };
          }
          return {
            ...entry,
            exists: false,
            columns: [],
            error: error instanceof Error ? error.message : String(error),
          };
        }
      }),
    );

    return reply.status(200).send({ enabled: true, ...base, tables });
  });

  // --- Schema for a single table -------------------------------------------
  app.get('/api/v1/admin/db/tables/:table/schema', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const entry = resolveTable(deps, (request.params as { table: string }).table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    try {
      const schema = await deps.aelioDbClient.getSchema(entry.name);
      return reply.status(200).send({ ...entry, columns: schema.columns });
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Scan rows (list, with optional filters) -----------------------------
  app.post('/api/v1/admin/db/tables/:table/scan', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const entry = resolveTable(deps, (request.params as { table: string }).table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }

    const body = (request.body ?? {}) as {
      limit?: number;
      filters?: QueryRequest['filters'];
      text?: QueryRequest['text'];
      sort?: { col: string; dir?: 'asc' | 'desc' };
    };

    const limit = Math.max(1, Math.min(body.limit ?? DEFAULT_SCAN_LIMIT, MAX_SCAN_LIMIT));
    const query: QueryRequest = { k: limit };
    if (body.filters && body.filters.length > 0) {
      query.filters = body.filters;
    }
    if (body.text && body.text.col && body.text.query) {
      query.text = body.text;
    }

    try {
      const result: ScanResponse = await deps.aelioDbClient.scanRows(entry.name, query);
      const sortCol = body.sort?.col ?? 'created_at';
      const dir = body.sort?.dir ?? 'desc';
      const rows = [...result.rows];
      rows.sort((a, b) => {
        const av = numericValue(a.values[sortCol]);
        const bv = numericValue(b.values[sortCol]);
        if (av !== null && bv !== null) {
          return dir === 'asc' ? av - bv : bv - av;
        }
        return dir === 'asc' ? a.row_id - b.row_id : b.row_id - a.row_id;
      });

      return reply.status(200).send({
        table: entry.name,
        count: rows.length,
        capped: rows.length >= limit,
        limit,
        rows: rows.map((row) => ({ row_id: row.row_id, values: lightenRow(row.values) })),
      });
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Get a single row (full, including embeddings) -----------------------
  app.get('/api/v1/admin/db/tables/:table/rows/:id', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const params = request.params as { table: string; id: string };
    const entry = resolveTable(deps, params.table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    const rowId = Number.parseInt(params.id, 10);
    if (!Number.isFinite(rowId)) {
      return reply.status(400).send({ error: 'Invalid row id' });
    }
    try {
      const row = await deps.aelioDbClient.getRow(entry.name, rowId);
      return reply.status(200).send(row);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Harness turn overview ------------------------------------------------
  app.get('/api/v1/admin/harness/turns/:turnId', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }

    const turnId = (request.params as { turnId: string }).turnId;
    try {
      const traceScan = await deps.aelioDbClient.scanRows(deps.config.aelioDb.tables.harness_traces, {
        k: 1000,
        filters: [{ col: 'turn_id', op: 'eq', value: queryUtf8(turnId) }],
      });
      const traces = traceScan.rows
        .map((row) => {
          const payloadText = stringValue(row.values.payload);
          return {
            rowId: row.row_id,
            turnId: stringValue(row.values.turn_id),
            sessionId: stringValue(row.values.session_id),
            kind: stringValue(row.values.kind),
            payload: parsePayload(payloadText),
            createdAt: numericValue(row.values.created_at) ?? 0,
          };
        })
        .sort((a, b) => a.createdAt - b.createdAt || a.rowId - b.rowId);

      const apiCalls = await listTurnApiCalls(
        { turnId, limit: 200 },
        {
          client: deps.aelioDbClient,
          table: deps.config.aelioDb.tables.turn_api_calls,
        },
      );

      return reply.status(200).send({
        turnId,
        sessionId: traces[0]?.sessionId ?? apiCalls[0]?.sessionId ?? '',
        decisions: {
          pathway: traces.find((trace) => trace.kind === 'pathway')?.payload ?? null,
          stance: traces.find((trace) => trace.kind === 'stance')?.payload ?? null,
          cache: traces.find((trace) => trace.kind === 'cache')?.payload ?? null,
          confirmation: traces.find((trace) => trace.kind === 'confirmation')?.payload ?? null,
          generic: traces.find((trace) => trace.kind === 'generic')?.payload ?? null,
          proactive: traces.find((trace) => trace.kind === 'proactive')?.payload ?? null,
        },
        prompt: traces.find((trace) => trace.kind === 'prompt')?.payload ?? null,
        reply: traces.find((trace) => trace.kind === 'reply')?.payload ?? null,
        traces,
        apiCalls,
      });
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Insert a row --------------------------------------------------------
  app.post('/api/v1/admin/db/tables/:table/rows', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const entry = resolveTable(deps, (request.params as { table: string }).table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    const values = (request.body as { values?: RowValues })?.values;
    if (!values || typeof values !== 'object') {
      return reply.status(400).send({ error: 'Body must be { values: { column: ApiValue } }' });
    }
    try {
      const result = await deps.aelioDbClient.insertRow(entry.name, values);
      return reply.status(201).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Update a row --------------------------------------------------------
  app.patch('/api/v1/admin/db/tables/:table/rows/:id', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const params = request.params as { table: string; id: string };
    const entry = resolveTable(deps, params.table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    const rowId = Number.parseInt(params.id, 10);
    if (!Number.isFinite(rowId)) {
      return reply.status(400).send({ error: 'Invalid row id' });
    }
    const values = (request.body as { values?: RowValues })?.values;
    if (!values || typeof values !== 'object') {
      return reply.status(400).send({ error: 'Body must be { values: { column: ApiValue } }' });
    }
    try {
      const result = await deps.aelioDbClient.updateRow(entry.name, rowId, values);
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Delete a row --------------------------------------------------------
  app.delete('/api/v1/admin/db/tables/:table/rows/:id', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    const params = request.params as { table: string; id: string };
    const entry = resolveTable(deps, params.table);
    if (!entry) {
      return reply.status(404).send({ error: 'Unknown table' });
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    const rowId = Number.parseInt(params.id, 10);
    if (!Number.isFinite(rowId)) {
      return reply.status(400).send({ error: 'Invalid row id' });
    }
    try {
      const result = await deps.aelioDbClient.deleteRow(entry.name, rowId);
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  // --- Admin: flush / compact ----------------------------------------------
  app.post('/api/v1/admin/db/flush', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    try {
      const result = await deps.aelioDbClient.flush();
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  app.post('/api/v1/admin/db/compact', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    if (!deps.config.aelioDb.enabled || !deps.aelioDbClient) {
      return aelioDbUnavailable(deps, reply);
    }
    try {
      const result = await deps.aelioDbClient.compact();
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });
}
