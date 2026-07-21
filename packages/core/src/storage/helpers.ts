import type { ApiValue } from '@aelio/sunjet-client';

export function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}

export function i64(value: number): ApiValue {
  return { type: 'i64', value };
}

export function f64(value: number): ApiValue {
  return { type: 'f64', value };
}

export function bool(value: boolean): ApiValue {
  return { type: 'bool', value };
}

export function readUtf8(values: Record<string, ApiValue>, key: string): string {
  const entry = values[key];
  if (entry?.type === 'utf8') {
    return entry.value;
  }
  return '';
}

export function readI64(values: Record<string, ApiValue>, key: string): number {
  const entry = values[key];
  if (entry?.type === 'i64') {
    return entry.value;
  }
  return 0;
}

export function readF64(values: Record<string, ApiValue>, key: string): number {
  const entry = values[key];
  if (entry?.type === 'f64') {
    return entry.value;
  }
  return 0;
}

export function readBool(values: Record<string, ApiValue>, key: string): boolean {
  const entry = values[key];
  if (entry?.type === 'bool') {
    return entry.value;
  }
  return false;
}

export function readVector(values: Record<string, ApiValue>, key: string): number[] | null {
  const entry = values[key];
  if (entry?.type === 'vector') {
    return entry.value;
  }
  return null;
}

export function parseJson<T>(raw: string, fallback: T): T {
  if (!raw) {
    return fallback;
  }
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}
