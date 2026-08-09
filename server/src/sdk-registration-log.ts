/**
 * Human-readable SDK registration dump for end-to-end rebuild.
 * Printed on Aelio server stdout, mirrored to the employer SDK terminal via
 * `registered` ack, and appended to data/aelio-sdk-connect.log.
 */

import { appendFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';

export type SdkRegistrationLogInput = {
  application: string;
  tenant: string;
  connectionId: string;
  remoteAddress?: string;
  sdkVersion: string;
  language: string;
  tools: Array<{ name: string; description?: string }>;
  states: Array<{ id: string; description?: string }>;
  flows: Array<{ id: string; description?: string; state?: string }>;
  policies: Array<{ id: string; description?: string }>;
};

const BORDER = '════════════════════════════════════════════════════════';

function bullet(label: string, items: string[]): string[] {
  if (items.length === 0) {
    return [`  ${label} (0): (none)`];
  }
  return [`  ${label} (${items.length}):`, ...items.map((item) => `    • ${item}`)];
}

function clip(text: string, max: number): string {
  const t = text.replace(/\s+/g, ' ').trim();
  return t.length > max ? `${t.slice(0, max)}…` : t;
}

/** Shared banner text (server + SDK client). */
export function formatSdkRegistrationBanner(input: SdkRegistrationLogInput): string {
  const toolNames = input.tools.map((t) => t.name);
  const stateLines = input.states.map((s) =>
    s.description ? `${s.id} — ${clip(s.description, 80)}` : s.id,
  );
  const flowLines = input.flows.map((f) => {
    const base = f.state ? `${f.id} [state=${f.state}]` : f.id;
    return f.description ? `${base} — ${clip(f.description, 80)}` : base;
  });
  const policyLines = input.policies.map((p) =>
    p.description ? `${p.id} — ${clip(p.description, 80)}` : p.id,
  );

  return [
    BORDER,
    `  ✓ ${input.application} successfully connected to Aelio`,
    `  tenant:     ${input.tenant}`,
    `  connection: ${input.connectionId}`,
    ...(input.remoteAddress ? [`  remote:     ${input.remoteAddress}`] : []),
    `  sdk:        ${input.language} ${input.sdkVersion}`,
    '',
    ...bullet('tools', toolNames),
    ...bullet('states', stateLines),
    ...bullet('flows', flowLines),
    ...bullet('policies', policyLines),
    BORDER,
  ].join('\n');
}

export function formatSdkRegisteredSummary(input: {
  application: string;
  tenant: string;
  tools: string[];
  states: string[];
  flows: string[];
  policies: string[];
}): string {
  return [
    BORDER,
    `  ✓ ${input.application} successfully connected to Aelio`,
    `  tenant:     ${input.tenant}`,
    '',
    ...bullet('tools', input.tools),
    ...bullet('states', input.states),
    ...bullet('flows', input.flows),
    ...bullet('policies', input.policies),
    BORDER,
  ].join('\n');
}

export function logSdkRegistrationSuccess(input: SdkRegistrationLogInput): void {
  const banner = formatSdkRegistrationBanner(input);
  process.stdout.write(`${banner}\n`);
  appendConnectLog(banner);
}

export function logSdkDisconnected(application: string, connectionId: string): void {
  const line = `${BORDER}\n  ✗ ${application} disconnected from Aelio (connection ${connectionId})\n${BORDER}`;
  process.stdout.write(`${line}\n`);
  appendConnectLog(line);
}

export function logSdkHeartbeat(input: {
  application: string;
  connectionId: string;
  tools: number;
  states: number;
  flows: number;
  policies: number;
}): void {
  const ts = new Date().toISOString().slice(11, 19);
  const line =
    `♥ [${ts}] heartbeat ok — ${input.application} ` +
    `(tools=${input.tools} states=${input.states} flows=${input.flows} policies=${input.policies}) ` +
    `conn=${input.connectionId.slice(0, 8)}…`;
  process.stdout.write(`${line}\n`);
}

export type CatalogNameSets = {
  tools: string[];
  states: string[];
  flows: string[];
  policies: string[];
};

export function diffCatalogNames(before: CatalogNameSets, after: CatalogNameSets): {
  added: CatalogNameSets;
  removed: CatalogNameSets;
  changed: boolean;
} {
  const diff = (prev: string[], next: string[]) => {
    const p = new Set(prev);
    const n = new Set(next);
    return {
      added: next.filter((x) => !p.has(x)),
      removed: prev.filter((x) => !n.has(x)),
    };
  };
  const tools = diff(before.tools, after.tools);
  const states = diff(before.states, after.states);
  const flows = diff(before.flows, after.flows);
  const policies = diff(before.policies, after.policies);
  const added = {
    tools: tools.added,
    states: states.added,
    flows: flows.added,
    policies: policies.added,
  };
  const removed = {
    tools: tools.removed,
    states: states.removed,
    flows: flows.removed,
    policies: policies.removed,
  };
  const changed =
    added.tools.length +
      added.states.length +
      added.flows.length +
      added.policies.length +
      removed.tools.length +
      removed.states.length +
      removed.flows.length +
      removed.policies.length >
    0;
  return { added, removed, changed };
}

export function logSdkCatalogPersist(input: {
  application: string;
  tenant: string;
  activated: CatalogNameSets;
  deactivated: CatalogNameSets;
}): void {
  const fmt = (label: string, names: string[]) =>
    names.length ? `  db ${label}: ${names.join(', ')}` : null;
  const lines = [
    fmt('+tools', input.activated.tools),
    fmt('+states', input.activated.states),
    fmt('+flows', input.activated.flows),
    fmt('+policies', input.activated.policies),
    fmt('−tools (active→false)', input.deactivated.tools),
    fmt('−states (active→false)', input.deactivated.states),
    fmt('−flows (active→false)', input.deactivated.flows),
    fmt('−policies (active→false)', input.deactivated.policies),
  ].filter((line): line is string => Boolean(line));
  if (lines.length === 0) {
    const line = `☰ catalog bag/db — ${input.application} @ ${input.tenant} (no active-flag changes)`;
    process.stdout.write(`${line}\n`);
    return;
  }
  const banner = [
    BORDER,
    `  ☰ ${input.application} catalog persisted (active soft-delete)`,
    `  tenant: ${input.tenant}`,
    ...lines,
    BORDER,
  ].join('\n');
  process.stdout.write(`${banner}\n`);
  appendConnectLog(banner);
}

export function logSdkCatalogUpdate(input: {
  application: string;
  tenant: string;
  connectionId: string;
  after: CatalogNameSets;
  added: CatalogNameSets;
  removed: CatalogNameSets;
}): void {
  const changeLines = (label: string, added: string[], removed: string[]) => {
    const lines: string[] = [];
    if (added.length) lines.push(`  + ${label}: ${added.join(', ')}`);
    if (removed.length) lines.push(`  − ${label}: ${removed.join(', ')}`);
    return lines;
  };
  const banner = [
    BORDER,
    `  ↻ ${input.application} catalog updated on Aelio`,
    `  tenant:     ${input.tenant}`,
    `  connection: ${input.connectionId}`,
    '',
    ...changeLines('tools', input.added.tools, input.removed.tools),
    ...changeLines('states', input.added.states, input.removed.states),
    ...changeLines('flows', input.added.flows, input.removed.flows),
    ...changeLines('policies', input.added.policies, input.removed.policies),
    '',
    ...bullet('tools now', input.after.tools),
    ...bullet('states now', input.after.states),
    ...bullet('flows now', input.after.flows),
    ...bullet('policies now', input.after.policies),
    BORDER,
  ].join('\n');
  process.stdout.write(`${banner}\n`);
  appendConnectLog(banner);
}

function connectLogPath(): string {
  if (process.env.AELIO_SDK_CONNECT_LOG) {
    return process.env.AELIO_SDK_CONNECT_LOG;
  }
  // `pnpm --filter @aelio/server` runs with cwd=server/; `pnpm start` uses repo root.
  const cwd = process.cwd();
  const root = cwd.endsWith('/server') || cwd.endsWith('\\server') ? join(cwd, '..') : cwd;
  return join(root, 'data', 'aelio-sdk-connect.log');
}

function appendConnectLog(banner: string): void {
  try {
    const logPath = connectLogPath();
    mkdirSync(dirname(logPath), { recursive: true });
    appendFileSync(logPath, `${new Date().toISOString()}\n${banner}\n\n`, 'utf8');
  } catch {
    // Logging must never break registration.
  }
}
