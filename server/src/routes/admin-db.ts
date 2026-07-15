import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify';
import { SunjetHttpError } from '@aelio/sunjet-client';
import type {
  ApiValue,
  QueryRequest,
  RowValues,
  ScanResponse,
  SchemaResponse,
} from '@aelio/sunjet-client';
import { resolvePublicDir } from '../paths.js';
import { requireSecret } from '../auth.js';
import type { RuntimeDeps } from '../runtime-deps.js';

type TableStatus = 'active' | 'mirror' | 'schema-only';

type TableCatalogEntry = {
  /** Stable config key. */
  key: string;
  /** Actual table name in Astrolobe (configurable). */
  name: string;
  label: string;
  description: string;
  status: TableStatus;
  writtenBy: string;
};

/**
 * Human-facing metadata for each logical Sunjet/Astrolobe table. Keys map to
 * `config.sunjet.tables.*`; the real table name is resolved at request time so
 * customised table names still work. `status` reflects what actually writes to
 * the table today (audit of the Aelio codebase).
 */
const TABLE_META: Array<{
  key: keyof RuntimeDeps['config']['sunjet']['tables'];
  label: string;
  description: string;
  status: TableStatus;
  writtenBy: string;
}> = [
  {
    key: 'messages',
    label: 'Messages',
    description: 'Raw conversation messages (L0). BM25 content + embedding. Dual-written with SQLite.',
    status: 'active',
    writtenBy: 'ConvoxMessageStore.appendMessage',
  },
  {
    key: 'conversations',
    label: 'Conversations',
    description: 'Rich per-turn telemetry: lifecycle, intent stack, flows, tools. Powers the telemetry dashboard.',
    status: 'active',
    writtenBy: 'appendConversationRecord',
  },
  {
    key: 'memories',
    label: 'Memories',
    description: 'Long-term customer memories. Schema exists but SQLite is authoritative today.',
    status: 'schema-only',
    writtenBy: '(SQLite authoritative)',
  },
  {
    key: 'compactions',
    label: 'Compactions',
    description: 'Tiered conversation summaries. Schema reserved; not yet populated.',
    status: 'schema-only',
    writtenBy: '(planned)',
  },
  {
    key: 'runtime_state',
    label: 'Runtime State',
    description: 'Ephemeral scoped state with TTL. Swept by sunjet-daemon.',
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
    description: 'Instruction→tool binding cache. Schema reserved; not yet populated.',
    status: 'schema-only',
    writtenBy: '(planned)',
  },
  {
    key: 'harness_suspensions',
    label: 'Harness Suspensions',
    description: 'Suspended plans awaiting user input. SQLite authoritative; mirrored here.',
    status: 'mirror',
    writtenBy: 'SuspensionStore.mirror',
  },
  {
    key: 'harness_ledger',
    label: 'Harness Ledger',
    description: 'Per-instruction idempotency ledger. SQLite only today.',
    status: 'schema-only',
    writtenBy: '(SQLite authoritative)',
  },
  {
    key: 'harness_traces',
    label: 'Harness Traces',
    description: 'Append-only harness trace firehose (plans, waves, gates, repairs). Fire-and-forget.',
    status: 'active',
    writtenBy: 'HarnessTracer.trace',
  },
];

const MAX_SCAN_LIMIT = 2000;
const DEFAULT_SCAN_LIMIT = 100;
const VECTOR_PREVIEW = 6;

function buildCatalog(deps: RuntimeDeps): TableCatalogEntry[] {
  const tables = deps.config.sunjet.tables;
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

/** 503 helper for when Sunjet is disabled or unreachable. */
function sunjetUnavailable(deps: RuntimeDeps, reply: FastifyReply) {
  return reply.status(503).send({
    enabled: false,
    message: deps.config.sunjet.enabled
      ? 'Sunjet is enabled but the client is not connected — check that ll-server is reachable.'
      : 'Sunjet is not enabled. Set sunjet.enabled: true in config.yaml and restart to browse the Astrolobe database.',
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

/** Translate a SunjetHttpError into an equivalent HTTP reply. */
function forwardError(error: unknown, reply: FastifyReply) {
  if (error instanceof SunjetHttpError) {
    return reply.status(error.status).send({ error: error.body || error.message });
  }
  const message = error instanceof Error ? error.message : String(error);
  return reply.status(502).send({ error: `Sunjet request failed: ${message}` });
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
      embedDim: deps.config.sunjet.embed_dim,
      url: deps.config.sunjet.url,
      dualWriteSqlite: deps.config.sunjet.dual_write_sqlite,
    };

    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return reply.status(200).send({
        enabled: false,
        ...base,
        tables: catalog.map((entry) => ({ ...entry, exists: false, columns: [] })),
      });
    }

    const client = deps.sunjetClient;
    const tables = await Promise.all(
      catalog.map(async (entry) => {
        try {
          const schema: SchemaResponse = await client.getSchema(entry.name);
          return { ...entry, exists: true, columns: schema.columns };
        } catch (error) {
          if (error instanceof SunjetHttpError && error.status === 404) {
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    try {
      const schema = await deps.sunjetClient.getSchema(entry.name);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
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
      const result: ScanResponse = await deps.sunjetClient.scanRows(entry.name, query);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    const rowId = Number.parseInt(params.id, 10);
    if (!Number.isFinite(rowId)) {
      return reply.status(400).send({ error: 'Invalid row id' });
    }
    try {
      const row = await deps.sunjetClient.getRow(entry.name, rowId);
      return reply.status(200).send(row);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    const values = (request.body as { values?: RowValues })?.values;
    if (!values || typeof values !== 'object') {
      return reply.status(400).send({ error: 'Body must be { values: { column: ApiValue } }' });
    }
    try {
      const result = await deps.sunjetClient.insertRow(entry.name, values);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
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
      const result = await deps.sunjetClient.updateRow(entry.name, rowId, values);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    const rowId = Number.parseInt(params.id, 10);
    if (!Number.isFinite(rowId)) {
      return reply.status(400).send({ error: 'Invalid row id' });
    }
    try {
      const result = await deps.sunjetClient.deleteRow(entry.name, rowId);
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
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    try {
      const result = await deps.sunjetClient.flush();
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });

  app.post('/api/v1/admin/db/compact', async (request, reply) => {
    if (!requireSecret(request, reply, secret)) {
      return reply;
    }
    if (!deps.config.sunjet.enabled || !deps.sunjetClient) {
      return sunjetUnavailable(deps, reply);
    }
    try {
      const result = await deps.sunjetClient.compact();
      return reply.status(200).send(result);
    } catch (error) {
      return forwardError(error, reply);
    }
  });
}
