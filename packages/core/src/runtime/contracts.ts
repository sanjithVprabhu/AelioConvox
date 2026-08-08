/**
 * The durable Aelio Runtime protocol. These are data contracts stored as JSON artifacts and
 * records in Aelio DB; they are intentionally not a programming language or free-form code.
 */

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type RuntimeEventKind = 'user_message' | 'provider_webhook' | 'scheduled' | 'continuation' | 'system';

export type RuntimeEventV1 = {
  apiVersion: 'aelio.runtime.event/v1';
  eventId: string;
  idempotencyKey: string;
  tenantId: string;
  subjectId: string;
  kind: RuntimeEventKind;
  payload: JsonValue;
  receivedAt: number;
};

export type RuntimeSnapshotV1 = {
  apiVersion: 'aelio.runtime.snapshot/v1';
  tenantId: string;
  subjectId: string;
  snapshotKey: string;
  revision: number;
  state: JsonValue;
  updatedAt: number;
};

export type RuntimeEffectV1 = {
  effectId: string;
  idempotencyKey: string;
  kind: string;
  payload: JsonValue;
};

export type RuntimeLedgerRecordV1 = {
  recordId: string;
  instanceId: string;
  kind: 'event_claimed' | 'decision' | 'step_started' | 'step_completed' | 'step_failed' | 'continuation' | 'effect';
  payload: JsonValue;
};

/** The conductor's small, auditable decision surface. */
export type RuntimeDecisionV1 =
  | { kind: 'reply'; text: string; state?: JsonValue; effects?: RuntimeEffectV1[] }
  | { kind: 'start_harness'; artifactId: string; version: string; input: JsonValue; state?: JsonValue }
  | { kind: 'continue_harness'; instanceId: string; input: JsonValue; state?: JsonValue }
  | { kind: 'await_input'; continuation: { token: string; prompt: string; expiresAt: number }; state?: JsonValue }
  | { kind: 'exit'; reason: string; state?: JsonValue };

/** Typed nodes are an approved runtime vocabulary. An artifact cannot execute arbitrary code. */
export type RuntimeNodeV1 =
  | { id: string; type: 'compute'; operation: 'add' | 'subtract' | 'multiply' | 'divide' | 'sum' | 'count' | 'average'; input: JsonValue; next?: string }
  | { id: string; type: 'memory'; operation: 'get' | 'set' | 'delete'; key: string; value?: JsonValue; next?: string }
  | { id: string; type: 'prompt'; promptId: string; version: string; input: JsonValue; next?: string }
  | { id: string; type: 'tool'; capability: string; input: JsonValue; next?: string }
  | { id: string; type: 'spawn'; artifactId: string; version: string; input: JsonValue; mode: 'inline' | 'child'; next?: string }
  /**
   * Await previously spawned detached children. `spawnNodes` names the `spawn` nodes whose child
   * instances must all reach a terminal status before the parent continues. A join that is not
   * ready parks the parent durably; each child's completion wakes it.
   */
  | { id: string; type: 'join'; spawnNodes: string[]; next?: string }
  | { id: string; type: 'wait'; token: string; prompt: string; expiresAt: number }
  | { id: string; type: 'effect'; effect: RuntimeEffectV1; next?: string }
  | { id: string; type: 'end'; output: JsonValue };

export type RuntimeArtifactV1 = {
  apiVersion: 'aelio.runtime.artifact/v1';
  artifactId: string;
  version: string;
  digest: string;
  status: 'draft' | 'approved' | 'retired';
  entryNodeId: string;
  nodes: RuntimeNodeV1[];
  createdAt: number;
};

/** A durable execution of one pinned approved artifact. Child instances point to their owner. */
export type RuntimeInstanceV1 = {
  instanceId: string;
  tenantId: string;
  subjectId: string;
  artifactId: string;
  artifactVersion: string;
  parentInstanceId?: string;
  status: 'pending' | 'running' | 'waiting' | 'completed' | 'failed' | 'cancelled';
  revision: number;
  state: JsonValue;
  updatedAt: number;
};

export function assertRuntimeEvent(event: RuntimeEventV1): void {
  if (event.apiVersion !== 'aelio.runtime.event/v1') throw new Error('unsupported runtime event version');
  for (const [name, value] of Object.entries({ eventId: event.eventId, idempotencyKey: event.idempotencyKey, tenantId: event.tenantId, subjectId: event.subjectId })) {
    if (!value.trim()) throw new Error(`runtime event ${name} is required`);
  }
  if (!Number.isFinite(event.receivedAt) || event.receivedAt <= 0) throw new Error('runtime event receivedAt must be epoch milliseconds');
}

export function assertRuntimeArtifact(artifact: RuntimeArtifactV1): void {
  if (artifact.apiVersion !== 'aelio.runtime.artifact/v1') throw new Error('unsupported runtime artifact version');
  if (!artifact.artifactId.trim() || !artifact.version.trim() || !artifact.digest.trim()) throw new Error('runtime artifact identity is incomplete');
  const ids = new Set<string>();
  for (const node of artifact.nodes) {
    if (!node.id.trim() || ids.has(node.id)) throw new Error(`runtime artifact has invalid or duplicate node '${node.id}'`);
    ids.add(node.id);
  }
  if (!ids.has(artifact.entryNodeId)) throw new Error('runtime artifact entry node does not exist');
  const spawnIds = new Set(artifact.nodes.filter((node) => node.type === 'spawn').map((node) => node.id));
  for (const node of artifact.nodes) {
    if ('next' in node && node.next && !ids.has(node.next)) throw new Error(`runtime artifact node '${node.id}' points to missing next node`);
    if (node.type !== 'join') continue;
    // A join may only await children this artifact actually spawns; anything else would let an
    // artifact park forever on an instance it does not own.
    if (node.spawnNodes.length === 0) throw new Error(`runtime artifact join '${node.id}' awaits no children`);
    for (const spawnNode of node.spawnNodes) {
      if (!spawnIds.has(spawnNode)) {
        throw new Error(`runtime artifact join '${node.id}' references '${spawnNode}', which is not a spawn node`);
      }
    }
  }
}
