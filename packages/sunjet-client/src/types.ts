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