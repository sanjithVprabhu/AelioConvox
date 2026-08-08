import { randomUUID } from 'node:crypto';
import type {
  JsonValue,
  RuntimeArtifactV1,
  RuntimeEffectV1,
  RuntimeLedgerRecordV1,
  RuntimeNodeV1,
} from './contracts.js';
import { assertRuntimeArtifact } from './contracts.js';

/** Everything needed to resume an artifact exactly where a crash or a wait left it. */
export type ArtifactCursor = {
  nodeId: string;
  /** Outputs of already-completed nodes, keyed by node id (`input` holds the artifact input). */
  values: Record<string, JsonValue>;
  steps: number;
};

export type ArtifactNodeContext = {
  /** Stable within one artifact instance, so an effect can be journaled idempotently. */
  nodeId: string;
};

export type RuntimeArtifactHost = {
  memory: {
    get(key: string): Promise<JsonValue | null>;
    set(key: string, value: JsonValue): Promise<void>;
    delete(key: string): Promise<boolean>;
  };
  runPrompt(input: { promptId: string; version: string; input: JsonValue }, node: ArtifactNodeContext): Promise<JsonValue>;
  runTool(input: { capability: string; input: JsonValue }, node: ArtifactNodeContext): Promise<JsonValue>;
  spawn(
    input: { artifactId: string; version: string; input: JsonValue; mode: 'inline' | 'child' },
    node: ArtifactNodeContext,
  ): Promise<JsonValue>;
  /**
   * Report whether every named child instance has reached a terminal status. `ready: false` parks
   * the parent; the runner wakes it when a child completes. A failed child is surfaced rather than
   * silently treated as success.
   */
  joinChildren(instanceIds: string[]): Promise<
    { ready: false; pending: string[] } | { ready: true; outputs: Record<string, JsonValue> }
  >;
};

export type ArtifactStepResult =
  | { status: 'advanced'; cursor: ArtifactCursor; effects: RuntimeEffectV1[]; ledger: RuntimeLedgerRecordV1[] }
  | { status: 'completed'; output: JsonValue; effects: RuntimeEffectV1[]; ledger: RuntimeLedgerRecordV1[] }
  | {
      status: 'awaiting_input';
      continuation: { token: string; prompt: string; expiresAt: number };
      cursor: ArtifactCursor;
      effects: RuntimeEffectV1[];
      ledger: RuntimeLedgerRecordV1[];
    }
  | {
      status: 'awaiting_children';
      pending: string[];
      cursor: ArtifactCursor;
      effects: RuntimeEffectV1[];
      ledger: RuntimeLedgerRecordV1[];
    };

export type ArtifactExecutionResult =
  | { status: 'completed'; output: JsonValue; effects: RuntimeEffectV1[]; ledger: RuntimeLedgerRecordV1[] }
  | {
      status: 'awaiting_input';
      continuation: { token: string; prompt: string; expiresAt: number };
      effects: RuntimeEffectV1[];
      ledger: RuntimeLedgerRecordV1[];
    };

export const DEFAULT_MAX_ARTIFACT_STEPS = 128;

export function startArtifactCursor(artifact: RuntimeArtifactV1, input: JsonValue): ArtifactCursor {
  assertRuntimeArtifact(artifact);
  return { nodeId: artifact.entryNodeId, values: { input }, steps: 0 };
}

/**
 * Executes exactly one approved node and returns the next cursor.
 *
 * Single-stepping is what makes an artifact durable: the caller commits the cursor, the ledger,
 * and any queued effects to Aelio DB after each node, so a crash resumes at the next node rather
 * than replaying side effects that already happened. It never makes an effect external itself —
 * effects are returned for the caller's transaction.
 */
export async function stepRuntimeArtifact(
  artifact: RuntimeArtifactV1,
  cursor: ArtifactCursor,
  host: RuntimeArtifactHost,
  maxSteps = DEFAULT_MAX_ARTIFACT_STEPS,
): Promise<ArtifactStepResult> {
  assertRuntimeArtifact(artifact);
  if (cursor.steps >= maxSteps) {
    throw new Error(`artifact '${artifact.artifactId}' exceeded ${maxSteps} nodes`);
  }
  const nodes = new Map(artifact.nodes.map((node) => [node.id, node]));
  const node = nodes.get(cursor.nodeId);
  if (!node) throw new Error(`artifact node '${cursor.nodeId}' is missing`);

  const ledger: RuntimeLedgerRecordV1[] = [record('step_started', { nodeId: node.id, type: node.type })];
  const effects: RuntimeEffectV1[] = [];
  const values = new Map(Object.entries(cursor.values));

  if (node.type === 'end') {
    ledger.push(record('step_completed', { nodeId: node.id }));
    return { status: 'completed', output: resolve(node.output, values), effects, ledger };
  }
  if (node.type === 'wait') {
    const continuation = { token: node.token, prompt: node.prompt, expiresAt: node.expiresAt };
    ledger.push(record('continuation', { nodeId: node.id, ...continuation }));
    return { status: 'awaiting_input', continuation, cursor, effects, ledger };
  }
  if (node.type === 'join') {
    const instanceIds = node.spawnNodes.map((spawnNode) => childInstanceIdOf(values, spawnNode));
    const joined = await host.joinChildren(instanceIds);
    if (!joined.ready) {
      ledger.push(record('continuation', { nodeId: node.id, awaitingChildren: joined.pending }));
      return { status: 'awaiting_children', pending: joined.pending, cursor, effects, ledger };
    }
    ledger.push(record('step_completed', { nodeId: node.id, children: instanceIds }));
    if (!node.next) throw new Error(`artifact node '${node.id}' has no next node or terminal type`);
    return {
      status: 'advanced',
      cursor: { nodeId: node.next, values: { ...cursor.values, [node.id]: joined.outputs }, steps: cursor.steps + 1 },
      effects,
      ledger,
    };
  }

  const output = await executeNode(node, values, host, effects);
  ledger.push(record('step_completed', { nodeId: node.id, output }));
  if (!node.next) throw new Error(`artifact node '${node.id}' has no next node or terminal type`);
  return {
    status: 'advanced',
    cursor: {
      nodeId: node.next,
      values: { ...cursor.values, [node.id]: output },
      steps: cursor.steps + 1,
    },
    effects,
    ledger,
  };
}

/**
 * Drive an artifact to its next terminal point in memory. Use this for pure artifacts and tests;
 * anything with external effects should be single-stepped so each node commits durably.
 */
export async function executeRuntimeArtifact(
  artifact: RuntimeArtifactV1,
  input: JsonValue,
  host: RuntimeArtifactHost,
  maxSteps = DEFAULT_MAX_ARTIFACT_STEPS,
): Promise<ArtifactExecutionResult> {
  let cursor = startArtifactCursor(artifact, input);
  const ledger: RuntimeLedgerRecordV1[] = [];
  const effects: RuntimeEffectV1[] = [];
  for (;;) {
    const step = await stepRuntimeArtifact(artifact, cursor, host, maxSteps);
    ledger.push(...step.ledger);
    effects.push(...step.effects);
    if (step.status === 'completed') return { status: 'completed', output: step.output, effects, ledger };
    if (step.status === 'awaiting_input') {
      return { status: 'awaiting_input', continuation: step.continuation, effects, ledger };
    }
    if (step.status === 'awaiting_children') {
      // In-memory driving has no way to make progress on a detached child; that requires the
      // durable runner. Failing loudly beats spinning on a join that can never resolve here.
      throw new Error(`artifact '${artifact.artifactId}' awaits children ${step.pending.join(', ')}; drive it durably`);
    }
    cursor = step.cursor;
  }
}

async function executeNode(
  node: Exclude<RuntimeNodeV1, { type: 'end' | 'wait' | 'join' }>,
  values: Map<string, JsonValue>,
  host: RuntimeArtifactHost,
  effects: RuntimeEffectV1[],
): Promise<JsonValue> {
  const context: ArtifactNodeContext = { nodeId: node.id };
  if (node.type === 'compute') return compute(node.operation, resolve(node.input, values));
  if (node.type === 'memory') {
    if (node.operation === 'get') return (await host.memory.get(node.key)) ?? null;
    if (node.operation === 'set') {
      await host.memory.set(node.key, resolve(node.value ?? null, values));
      return true;
    }
    return host.memory.delete(node.key);
  }
  if (node.type === 'prompt') {
    return host.runPrompt({ promptId: node.promptId, version: node.version, input: resolve(node.input, values) }, context);
  }
  if (node.type === 'tool') return host.runTool({ capability: node.capability, input: resolve(node.input, values) }, context);
  if (node.type === 'spawn') {
    return host.spawn(
      { artifactId: node.artifactId, version: node.version, input: resolve(node.input, values), mode: node.mode },
      context,
    );
  }
  effects.push(node.effect);
  return { effectId: node.effect.effectId, queued: true };
}

function compute(operation: Extract<RuntimeNodeV1, { type: 'compute' }>['operation'], input: JsonValue): JsonValue {
  const values = Array.isArray(input) ? input : typeof input === 'object' && input !== null && Array.isArray(input.values) ? input.values : null;
  if (!values || values.some((value) => typeof value !== 'number' || !Number.isFinite(value))) {
    throw new Error(`compute.${operation} requires an array of finite numbers`);
  }
  const numbers = values as number[];
  if (operation === 'count') return numbers.length;
  if (numbers.length === 0) throw new Error(`compute.${operation} requires at least one value`);
  if (operation === 'sum') return numbers.reduce((sum, value) => sum + value, 0);
  if (operation === 'average') return numbers.reduce((sum, value) => sum + value, 0) / numbers.length;
  if (numbers.length !== 2) throw new Error(`compute.${operation} requires exactly two values`);
  const left = numbers[0]!;
  const right = numbers[1]!;
  if (operation === 'add') return left + right;
  if (operation === 'subtract') return left - right;
  if (operation === 'multiply') return left * right;
  if (right === 0) throw new Error('compute.divide cannot divide by zero');
  return left / right;
}

/** Resolves only explicit `{ "$ref": "node-id" }` values; all other JSON stays data. */
function resolve(value: JsonValue, values: Map<string, JsonValue>): JsonValue {
  if (typeof value === 'object' && value !== null && !Array.isArray(value) && Object.keys(value).length === 1 && typeof value.$ref === 'string') {
    const resolved = values.get(value.$ref);
    if (resolved === undefined) throw new Error(`artifact reference '${value.$ref}' is unavailable`);
    return resolved;
  }
  if (Array.isArray(value)) return value.map((entry) => resolve(entry, values));
  if (typeof value === 'object' && value !== null) return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, resolve(entry, values)]));
  return value;
}

/** A spawn node's recorded output is `{ instanceId, ... }`; a join reads the identity back out. */
function childInstanceIdOf(values: Map<string, JsonValue>, spawnNodeId: string): string {
  const spawned = values.get(spawnNodeId);
  if (!spawned || typeof spawned !== 'object' || Array.isArray(spawned) || typeof spawned.instanceId !== 'string') {
    throw new Error(`join references spawn node '${spawnNodeId}', which has not produced a child instance`);
  }
  return spawned.instanceId;
}

function record(kind: RuntimeLedgerRecordV1['kind'], payload: JsonValue): RuntimeLedgerRecordV1 {
  return { recordId: randomUUID(), instanceId: '', kind, payload };
}
