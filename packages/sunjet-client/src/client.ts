import type {
  ColumnSpec,
  QueryRequest,
  QueryResponse,
  RowValues,
  ScanResponse,
  SchemaResponse,
  SunjetClientOptions,
  TransactionMutation,
  TransactionPrecondition,
  TransactionResponse,
} from './types.js';

export class SunjetHttpError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: string,
    message?: string,
  ) {
    super(message ?? `Sunjet HTTP ${status}: ${body}`);
    this.name = 'SunjetHttpError';
  }
}

export class SunjetClient {
  private readonly baseUrl: string;
  private readonly apiKey?: string;
  private readonly timeoutMs: number;

  constructor(options: SunjetClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/$/, '');
    this.apiKey = options.apiKey?.trim() || undefined;
    this.timeoutMs = options.timeoutMs ?? 30_000;
  }

  async health(): Promise<{ status: string; service: string }> {
    return this.request('GET', '/v1/health');
  }

  async createTable(name: string, columns: ColumnSpec[]): Promise<{ table_id: number }> {
    return this.request('POST', '/v1/tables', { name, columns });
  }

  async getSchema(table: string): Promise<SchemaResponse> {
    return this.request('GET', `/v1/tables/${encodeURIComponent(table)}/schema`);
  }

  async addColumns(table: string, columns: ColumnSpec[]): Promise<{ applied: boolean }> {
    return this.request('POST', `/v1/tables/${encodeURIComponent(table)}/columns`, { columns });
  }

  async ensureTable(name: string, columns: ColumnSpec[]): Promise<void> {
    try {
      const existing = await this.getSchema(name);
      const actual = new Map(existing.columns.map((column) => [column.name, column]));
      const missingOrIncompatible = columns.filter((expected) => {
        const found = actual.get(expected.name);
        return !found || found.kind !== expected.kind || (expected.kind === 'vector' && found.dim !== expected.dim);
      });
      const incompatible = missingOrIncompatible.filter((expected) => actual.has(expected.name));
      if (incompatible.length > 0) {
        throw new Error(
          `Aelio DB table '${name}' does not match the required schema: ` +
          incompatible.map((column) => `${column.name}:${column.kind}${column.dim ? `(${column.dim})` : ''}`).join(', ') +
          '. Apply an explicit Aelio DB catalog migration before starting this runtime.',
        );
      }
      const missing = missingOrIncompatible.filter((expected) => !actual.has(expected.name));
      if (missing.length > 0) await this.addColumns(name, missing);
    } catch (error) {
      if (error instanceof SunjetHttpError && error.status === 404) {
        await this.createTable(name, columns);
        return;
      }
      throw error;
    }
  }

  async insertRow(table: string, values: RowValues): Promise<{ row_id: number }> {
    return this.request('POST', `/v1/tables/${encodeURIComponent(table)}/rows`, { values });
  }

  /**
   * Commit all mutations together, or commit none. Use this for runtime event claims and the
   * corresponding state, ledger, and outbox writes; do not split that transition into several
   * single-row requests.
   */
  async transact(
    mutations: TransactionMutation[],
    preconditions: TransactionPrecondition[] = [],
  ): Promise<TransactionResponse> {
    return this.request('POST', '/v1/transactions', { preconditions, mutations });
  }

  async updateRow(
    table: string,
    rowId: number,
    values: RowValues,
  ): Promise<{ applied: boolean }> {
    return this.request(
      'PATCH',
      `/v1/tables/${encodeURIComponent(table)}/rows/${rowId}`,
      { values },
    );
  }

  async deleteRow(table: string, rowId: number): Promise<{ applied: boolean }> {
    return this.request('DELETE', `/v1/tables/${encodeURIComponent(table)}/rows/${rowId}`);
  }

  async getRow(table: string, rowId: number): Promise<{ row_id: number; values: RowValues }> {
    return this.request('GET', `/v1/tables/${encodeURIComponent(table)}/rows/${rowId}`);
  }

  async scanRows(table: string, query: QueryRequest): Promise<ScanResponse> {
    return this.request('POST', `/v1/tables/${encodeURIComponent(table)}/scan`, query);
  }

  async query(table: string, query: QueryRequest): Promise<QueryResponse> {
    return this.request('POST', `/v1/tables/${encodeURIComponent(table)}/query`, query);
  }

  async flush(): Promise<{ ok: boolean }> {
    return this.request('POST', '/v1/admin/flush');
  }

  async compact(): Promise<{ ok: boolean }> {
    return this.request('POST', '/v1/admin/compact');
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);

    try {
      const headers: Record<string, string> = {};
      if (body !== undefined) {
        headers['content-type'] = 'application/json';
      }
      if (this.apiKey) {
        headers.authorization = `Bearer ${this.apiKey}`;
      }

      const response = await fetch(`${this.baseUrl}${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
      });

      const text = await response.text();
      if (!response.ok) {
        throw new SunjetHttpError(response.status, text);
      }

      if (!text) {
        return {} as T;
      }

      return JSON.parse(text) as T;
    } catch (error) {
      if (error instanceof SunjetHttpError) {
        throw error;
      }
      if (error instanceof Error && error.name === 'AbortError') {
        throw new Error(`Sunjet request timed out after ${this.timeoutMs}ms`);
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }
}
