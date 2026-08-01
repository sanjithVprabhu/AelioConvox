import type { ApiValue, AelioDbClient } from '@aelio/db-client';
import { embed } from '../analyst/embeddings.js';
import { normalizeEmbedding } from '../storage/messages.js';

export type TraceKind =
  | 'pathway'
  | 'stance'
  | 'prompt'
  | 'reply'
  | 'cache'
  | 'confirmation'
  | 'generic'
  | 'proactive'
  | 'temporal'
  | 'evidence'
  | 'plan'
  | 'bind'
  | 'wave'
  | 'gate'
  | 'repair'
  | 'suspend'
  | 'resume'
  | 'synthesis'
  | 'budget';

export type HarnessTracerConfig = {
  client: AelioDbClient;
  table: string;
  tenant: string;
  embedDim: number;
};

function utf8(value: string): ApiValue {
  return { type: 'utf8', value };
}
function i64(value: number): ApiValue {
  return { type: 'i64', value };
}

/**
 * Append-only trace firehose into AelioDb (tier 0 — the existing compaction
 * daemon rolls traces up the L0–L3 ladder like any other tiered content).
 * Strictly fire-and-forget: tracing never blocks or fails a turn, and traces
 * are never fed back into prompts — they exist for audit, replay, and the
 * telemetry surface. Only `plan` rows get an embedding (they're the ones worth
 * semantic search); the rest store payload text alone.
 */
export class HarnessTracer {
  constructor(private readonly config: HarnessTracerConfig | null) {}

  trace(input: {
    turnId: string;
    sessionId: string;
    kind: TraceKind;
    payload: unknown;
  }): void {
    const config = this.config;
    if (!config) {
      return;
    }
    void (async () => {
      try {
        const payloadText = JSON.stringify(input.payload);
        const vector =
          input.kind === 'plan'
            ? normalizeEmbedding(await embed(payloadText.slice(0, 2000)), config.embedDim)
            : new Array<number>(config.embedDim).fill(0);
        await config.client.insertRow(config.table, {
          turn_id: utf8(input.turnId),
          session_id: utf8(input.sessionId),
          tenant: utf8(config.tenant),
          kind: utf8(input.kind),
          payload: utf8(payloadText),
          embedding: { type: 'vector', value: vector },
          tier: i64(0),
          created_at: i64(Date.now()),
        });
      } catch {
        // Lossy by design.
      }
    })();
  }
}
