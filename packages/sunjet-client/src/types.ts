export type ColumnKind =
  | 'bool'
  | 'i64'
  | 'f64'
  | 'utf8'
  | 'timestamp'
  | 'text'
  | 'edge'
  | 'vector';

export type ColumnSpec = {
  name: string;
  kind: ColumnKind;
  dim?: number;
};

export type ApiValue =
  | { type: 'null' }
  | { type: 'bool'; value: boolean }
  | { type: 'i64'; value: number }
  | { type: 'f64'; value: number }
  | { type: 'utf8'; value: string }
  | { type: 'vector'; value: number[] }
  | { type: 'edges'; value: number[] }
  | { type: 'embed'; value: string };

export type RowValues = Record<string, ApiValue>;

/** One member of an atomic Aelio DB write transaction. */
export type TransactionMutation =
  | { op: 'insert'; table: string; values: RowValues }
  | { op: 'update'; table: string; row_id: number; values: RowValues }
  | { op: 'delete'; table: string; row_id: number };

export type TransactionPrecondition =
  | { kind: 'absent'; table: string; equals: RowValues }
  | { kind: 'row_matches'; table: string; row_id: number; equals: RowValues };

export type TransactionResponse = {
  /** False means a condition failed and Aelio DB appended no transaction. */
  applied: boolean;
  /** Durable WAL commit position shared by every mutation when `applied` is true. */
  commit_lsn?: number;
  results: Array<{ op: 'insert' | 'update' | 'delete'; row_id: number }>;
};

export type FilterClause = {
  col: string;
  op: 'eq' | 'ne' | 'gt' | 'ge' | 'lt' | 'le';
  value: ApiValue;
};

export type QueryRequest = {
  k: number;
  vector?: { col: string; query: number[] };
  semantic?: { col: string; text: string };
  text?: { col: string; query: string };
  filters?: FilterClause[];
  graph?: { col: string; seeds: number[]; depth: number };
};

export type RowValueResponse = {
  row_id: number;
  values: RowValues;
};

export type ScanResponse = {
  rows: RowValueResponse[];
};

export type QueryResponse = {
  results: Array<{ row_id: number; score: number }>;
};

export type SchemaResponse = {
  table: string;
  columns: Array<{ name: string; kind: string; dim?: number }>;
};

export type SunjetClientOptions = {
  baseUrl: string;
  apiKey?: string;
  timeoutMs?: number;
};
