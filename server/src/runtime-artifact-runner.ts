import { createHash } from 'node:crypto';
import {
  DEFAULT_MAX_ARTIFACT_STEPS,
  startArtifactCursor,
  stepRuntimeArtifact,
  type AelioRuntimeStore,
  type ArtifactCursor,
  type ArtifactNodeContext,
  type JsonValue,
  type RuntimeArtifactHost,
  type RuntimeArtifactV1,
  type RuntimeEffectV1,
  type RuntimeInstanceRecord,
  type RuntimeLedgerRecordV1,
  type RuntimeOutboxEffect,
} from '@aelio/core';
import type { RuntimeDeps } from './runtime-deps.js';

/** How deep an artifact may nest inline children before the runtime refuses to go further. */
const MAX_SPAWN_DEPTH = 4;

/**
 * Executes a persisted `harness.start` effect against its pinned approved artifact.
 *
 * The artifact is single-stepped: after every node the instance's cursor, ledger records, and any
 * queued effects are committed to Aelio DB in one conditional transaction. A crash therefore
 * resumes at the next node instead of replaying nodes that already ran, and every externally
 * visible node (tool, prompt) additionally passes through the intent/result effect journal so a
 * crash *inside* a node cannot silently repeat it.
 */
export async function runHarnessEffect(deps: RuntimeDeps, effect: RuntimeOutboxEffect): Promise<void> {
  const payload = asObject(effect.payload);
  const artifactId = requiredString(payload, 'artifactId');
  const version = requiredString(payload, 'version');
  const instanceId = requiredString(payload, 'instanceId');
  const input = payload.input;
  if (input === undefined) throw new Error('harness.start requires input');
  const store = requireStore(deps);

  const artifact = await store.loadArtifact(artifactId, version);
  if (!artifact) throw new Error(`approved harness artifact ${artifactId}@${version} was not found`);

  let instance = await store.loadInstance(effect.tenantId, instanceId);
  if (!instance) {
    await store.createInstance({
      instanceId,
      tenantId: effect.tenantId,
      subjectId: effect.subjectId,
      artifactId,
      artifactVersion: version,
      status: 'running',
      state: { input, cursor: startArtifactCursor(artifact, input) as unknown as JsonValue },
    });
    instance = await store.loadInstance(effect.tenantId, instanceId);
  }
  if (!instance) throw new Error(`could not create harness instance ${instanceId}`);
  if (instance.status !== 'running' && instance.status !== 'pending') return;

  await driveInstance(deps, store, artifact, instance, effect.eventId, 0);
}

/**
 * Wake a parent parked on a join. It is a no-op unless the instance is genuinely waiting, so a
 * redelivered wake — or several children finishing at once — cannot double-drive it.
 */
export async function resumeHarnessInstance(deps: RuntimeDeps, effect: RuntimeOutboxEffect): Promise<void> {
  const store = requireStore(deps);
  const instanceId = requiredString(asObject(effect.payload), 'instanceId');
  const instance = await store.loadInstance(effect.tenantId, instanceId);
  if (!instance || instance.status !== 'waiting') return;
  if (!asObject(instance.state).awaitingChildren) return;

  const artifact = await store.loadArtifact(instance.artifactId, instance.artifactVersion);
  if (!artifact) throw new Error(`pinned artifact ${instance.artifactId}@${instance.artifactVersion} was not found`);
  // Re-enter as running so the join re-evaluates; if children are still pending it parks again.
  const running = await store.commitInstanceTransition({
    instance, eventId: effect.eventId, status: 'running', state: instance.state, ledger: [],
  });
  if (!running) return;
  const reloaded = await store.loadInstance(effect.tenantId, instanceId);
  if (!reloaded) return;
  await driveInstance(deps, store, artifact, reloaded, effect.eventId, 0);
}

async function driveInstance(
  deps: RuntimeDeps,
  store: AelioRuntimeStore,
  artifact: RuntimeArtifactV1,
  start: RuntimeInstanceRecord,
  eventId: string,
  depth: number,
): Promise<JsonValue> {
  let instance = start;
  const input = asObject(instance.state).input ?? null;
  let cursor = readCursor(instance.state) ?? startArtifactCursor(artifact, input);

  for (let guard = 0; guard <= DEFAULT_MAX_ARTIFACT_STEPS; guard += 1) {
    const host = createHost(deps, store, instance, eventId, depth);
    const step = await stepRuntimeArtifact(artifact, cursor, host);
    const ledger: RuntimeLedgerRecordV1[] = step.ledger;
    const effects: RuntimeEffectV1[] = [...step.effects];

    if (step.status === 'completed') {
      // A finished child wakes its parent's join. The wake is an ordinary outbox effect, so it is
      // committed in the same transaction that marks the child complete — it cannot be lost.
      if (instance.parentInstanceId) {
        effects.push({
          effectId: `${instance.instanceId}:parent-wake`,
          idempotencyKey: `harness.resume:${instance.parentInstanceId}:${instance.instanceId}`,
          kind: 'harness.resume',
          payload: { instanceId: instance.parentInstanceId },
        });
      }
      await commit(store, instance, eventId, 'completed', { input, output: step.output }, ledger, effects);
      return step.output;
    }

    if (step.status === 'awaiting_children') {
      await commit(
        store, instance, eventId, 'waiting',
        { input, cursor: cursor as unknown as JsonValue, awaitingChildren: step.pending },
        ledger, effects,
      );
      return null;
    }

    if (step.status === 'awaiting_input') {
      effects.push({
        effectId: `${instance.instanceId}:continuation:${step.continuation.token}`,
        idempotencyKey: `continuation:${instance.instanceId}:${step.continuation.token}`,
        kind: 'continuation.create',
        payload: { ...step.continuation, instanceId: instance.instanceId },
      });
      await commit(
        store, instance, eventId, 'waiting',
        { input, cursor: cursor as unknown as JsonValue, continuation: step.continuation },
        ledger, effects,
      );
      return null;
    }

    cursor = step.cursor;
    await commit(store, instance, eventId, 'running', { input, cursor: cursor as unknown as JsonValue }, ledger, effects);
    const reloaded = await store.loadInstance(instance.tenantId, instance.instanceId);
    if (!reloaded) throw new Error(`harness instance ${instance.instanceId} disappeared mid-execution`);
    instance = reloaded;
  }
  throw new Error(`harness instance ${instance.instanceId} exceeded its step budget`);
}

async function commit(
  store: AelioRuntimeStore,
  instance: RuntimeInstanceRecord,
  eventId: string,
  status: 'running' | 'waiting' | 'completed',
  state: Record<string, JsonValue>,
  ledger: RuntimeLedgerRecordV1[],
  effects: RuntimeEffectV1[],
): Promise<void> {
  // Artifact memory is mutated by the host during the step; fold it in here so a `memory.set`
  // becomes durable at exactly the same commit point as the node that made it.
  const merged: Record<string, JsonValue> = { ...state, memory: readMemory(instance.state) };
  const applied = await store.commitInstanceTransition({
    instance, eventId, status, state: merged as unknown as JsonValue, ledger, effects,
  });
  // Losing the CAS means another worker owns this instance. Throwing hands the outbox record back
  // to the lease/retry reaper rather than letting two workers interleave the same artifact.
  if (!applied) throw new Error(`harness instance ${instance.instanceId} changed while executing`);
}

/**
 * The approved node adapters.
 *
 * `memory` is instance-scoped and lives in the instance state, so a set is durable at the same
 * commit point as the step that made it. `prompt` and `tool` are externally visible and run under
 * the effect journal, keyed deterministically by instance + node + argument hash.
 */
function createHost(
  deps: RuntimeDeps,
  store: AelioRuntimeStore,
  instance: RuntimeInstanceRecord,
  eventId: string,
  depth: number,
): RuntimeArtifactHost {
  const memory = new Map(Object.entries(readMemory(instance.state)));

  const journaled = async (
    node: ArtifactNodeContext,
    kind: string,
    request: JsonValue,
    perform: () => Promise<JsonValue>,
  ): Promise<JsonValue> => {
    const key = `${instance.instanceId}:${node.nodeId}:${digest(request)}`;
    const claim = await store.beginEffect({
      key, kind, request, eventId,
      tenantId: instance.tenantId, subjectId: instance.subjectId, instanceId: instance.instanceId,
    });
    if (claim.state === 'completed') return claim.result;
    if (claim.state === 'unknown') {
      // A prior attempt was interrupted after dispatch. The outcome is genuinely unknown, so the
      // only safe move is to stop and let reconciliation decide — never to try again.
      throw new Error(`artifact effect ${key} has an unknown outcome and needs reconciliation`);
    }
    const result = await perform();
    await store.recordEffectResult({
      key, result, eventId,
      tenantId: instance.tenantId, subjectId: instance.subjectId, instanceId: instance.instanceId,
    });
    return result;
  };

  return {
    memory: {
      get: async (key) => memory.get(key) ?? null,
      set: async (key, value) => { memory.set(key, value); syncMemory(instance, memory); },
      delete: async (key) => { const had = memory.delete(key); syncMemory(instance, memory); return had; },
    },

    runPrompt: async ({ promptId, version, input }, node) =>
      journaled(node, 'artifact.prompt', { promptId, version, input }, async () => {
        const prompt = await store.loadPromptArtifact(promptId, version);
        if (!prompt) throw new Error(`approved prompt artifact ${promptId}@${version} was not found`);
        const slots = asObject(input);
        const missing = prompt.template.slots.filter((slot) => slots[slot] === undefined);
        if (missing.length > 0) {
          throw new Error(`prompt ${promptId}@${version} is missing required slots: ${missing.join(', ')}`);
        }
        const completion = await deps.llm.complete({
          system: render(prompt.template.system, prompt.template.slots, slots),
          messages: [{ role: 'user', content: render(prompt.template.user, prompt.template.slots, slots) }],
          tools: [],
          model: deps.config.llm.model,
          maxTokens: prompt.template.maxOutputTokens ?? deps.config.llm.max_tokens,
          temperature: 0,
          toolChoice: { type: 'none' },
          telemetry: { purpose: 'chat_completion' },
        });
        return { text: completion.text, promptDigest: prompt.digest };
      }),

    runTool: async ({ capability, input }, node) =>
      journaled(node, 'artifact.tool', { capability, input }, async () => {
        const fn = deps.sdkBridge.getFunctions().find((entry) => entry.name === capability);
        if (!fn) throw new Error(`artifact tool '${capability}' is not in the tenant registry`);
        const result = await deps.sdkBridge.invoke(capability, asObject(input), {
          customerId: instance.subjectId,
          sessionId: `${instance.tenantId}:${instance.subjectId}`,
          channel: 'sdk',
          channelAddress: `sdk:${instance.subjectId}`,
        });
        if (!result.ok) throw new Error(`artifact tool '${capability}' failed: ${result.error ?? 'unknown error'}`);
        return (result.data ?? null) as JsonValue;
      }),

    spawn: async ({ artifactId, version, input, mode }, node) => {
      if (depth >= MAX_SPAWN_DEPTH) {
        throw new Error(`artifact spawn exceeded the maximum depth of ${MAX_SPAWN_DEPTH}`);
      }
      const child = await store.loadArtifact(artifactId, version);
      if (!child) throw new Error(`approved harness artifact ${artifactId}@${version} was not found`);
      // Child ids are derived from the parent node, not random, so a retried parent step joins the
      // existing child instead of forking a second one.
      const childInstanceId = `${instance.instanceId}:${node.nodeId}`;

      if (mode === 'child') {
        // Detached: create the child and queue its own start effect. Both are keyed off the parent
        // node, so a retried parent step joins the existing child rather than forking a second one.
        await store.createInstance({
          instanceId: childInstanceId, tenantId: instance.tenantId, subjectId: instance.subjectId,
          artifactId, artifactVersion: version, parentInstanceId: instance.instanceId,
          status: 'pending', state: { input, cursor: startArtifactCursor(child, input) as unknown as JsonValue },
        });
        await store.enqueueEffect({
          tenantId: instance.tenantId, subjectId: instance.subjectId, eventId,
          effect: {
            effectId: `${childInstanceId}:start`,
            idempotencyKey: `harness.start:${childInstanceId}`,
            kind: 'harness.start',
            payload: { artifactId, version, input, instanceId: childInstanceId },
          },
        });
        return { instanceId: childInstanceId, mode: 'child' };
      }

      await store.createInstance({
        instanceId: childInstanceId, tenantId: instance.tenantId, subjectId: instance.subjectId,
        artifactId, artifactVersion: version, parentInstanceId: instance.instanceId,
        status: 'running', state: { input, cursor: startArtifactCursor(child, input) as unknown as JsonValue },
      });
      const childInstance = await store.loadInstance(instance.tenantId, childInstanceId);
      if (!childInstance) throw new Error(`could not create child instance ${childInstanceId}`);
      if (childInstance.status === 'completed') return asObject(childInstance.state).output ?? null;
      const output = await driveInstance(deps, store, child, childInstance, eventId, depth + 1);
      return { instanceId: childInstanceId, mode: 'inline', output };
    },

    joinChildren: async (instanceIds) => {
      const children = await store.listChildInstances(instance.tenantId, instance.instanceId);
      const byId = new Map(children.map((entry) => [entry.instanceId, entry]));
      const pending: string[] = [];
      const outputs: Record<string, JsonValue> = {};
      for (const instanceId of instanceIds) {
        const child = byId.get(instanceId);
        if (!child || child.status === 'pending' || child.status === 'running' || child.status === 'waiting') {
          pending.push(instanceId);
          continue;
        }
        // A failed or cancelled child is not success. Surfacing it stops the parent rather than
        // letting a downstream node read a missing output as an empty one.
        if (child.status !== 'completed') {
          throw new Error(`child instance ${instanceId} ended as '${child.status}'; the join cannot complete`);
        }
        outputs[instanceId] = asObject(child.state).output ?? null;
      }
      return pending.length > 0 ? { ready: false, pending } : { ready: true, outputs };
    },
  };
}

/**
 * Renders only declared slots, and only as data. A slot value is JSON-encoded rather than pasted
 * raw, so a customer string can never terminate the template and read as instruction text.
 */
function render(template: string, slots: string[], values: Record<string, JsonValue>): string {
  return slots.reduce(
    (text, slot) => text.split(`{{${slot}}}`).join(JSON.stringify(values[slot] ?? null)),
    template,
  );
}

function digest(value: JsonValue): string {
  return createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex').slice(0, 32);
}

function canonical(value: JsonValue): JsonValue {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([k, v]) => [k, canonical(v)]),
    );
  }
  return value;
}

function readCursor(state: JsonValue): ArtifactCursor | null {
  const cursor = asObject(state).cursor;
  if (!cursor || typeof cursor !== 'object' || Array.isArray(cursor)) return null;
  const nodeId = cursor.nodeId;
  const values = cursor.values;
  if (typeof nodeId !== 'string' || !values || typeof values !== 'object' || Array.isArray(values)) return null;
  return { nodeId, values, steps: typeof cursor.steps === 'number' ? cursor.steps : 0 };
}

function readMemory(state: JsonValue): Record<string, JsonValue> {
  const memory = asObject(state).memory;
  return memory && typeof memory === 'object' && !Array.isArray(memory) ? memory : {};
}

/** Memory writes ride along on the state the next `commit` persists. */
function syncMemory(instance: RuntimeInstanceRecord, memory: Map<string, JsonValue>): void {
  instance.state = { ...asObject(instance.state), memory: Object.fromEntries(memory) };
}

function requireStore(deps: RuntimeDeps): AelioRuntimeStore {
  if (!deps.runtimeStore) throw new Error('Aelio Runtime requires configured Aelio DB storage');
  return deps.runtimeStore;
}

function asObject(value: JsonValue): Record<string, JsonValue> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  return value;
}

function requiredString(value: Record<string, JsonValue>, key: string): string {
  const found = value[key];
  if (typeof found !== 'string' || !found) throw new Error(`effect payload is missing ${key}`);
  return found;
}
