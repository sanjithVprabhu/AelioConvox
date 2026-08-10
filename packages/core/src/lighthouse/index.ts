import type { FunctionDefinition } from '@aelio/protocol';
import type { SunjetClient } from '@aelio/sunjet-client';
import { cosineSimilarity, embed } from '../analyst/embeddings.js';
import type { SdkBridge } from '../sdk-bridge/types.js';
import { registryHash, type RegistrySnapshot } from './hash.js';
import { LighthouseMirror, toolDescriptor, type ToolSearchHit } from './mirror.js';
import { buildCapabilityBrief } from './taxonomy.js';

export { registryHash, type RegistrySnapshot } from './hash.js';
export { buildCapabilityBrief } from './taxonomy.js';
export { LighthouseMirror, type ToolSearchHit } from './mirror.js';

export type LighthouseConfig = {
  bridge: SdkBridge;
  tenant: string;
  sunjet?: {
    client: SunjetClient;
    toolsTable: string;
    capabilitiesTable: string;
    embedDim: number;
  };
};

/**
 * Lighthouse: the harness's single read model over everything the SDK has
 * registered. The in-memory bridge stays authoritative for liveness and
 * schemas; Lighthouse adds the derived artifacts the planner and binder need —
 * a stable registry hash, the capability brief, and (when Sunjet is up) an
 * indexed mirror for semantic tool search with prerequisite-graph expansion.
 *
 * All derived artifacts are keyed by the registry hash: nothing is recomputed
 * on reconnects that don't change the registry, and everything is invalidated
 * the moment it does.
 */
export class LighthouseService {
  private readonly mirror: LighthouseMirror | null;
  private hash = '';
  private brief = '';
  private syncing: Promise<void> | null = null;

  constructor(private readonly config: LighthouseConfig) {
    this.mirror = config.sunjet
      ? new LighthouseMirror({
          client: config.sunjet.client,
          toolsTable: config.sunjet.toolsTable,
          capabilitiesTable: config.sunjet.capabilitiesTable,
          embedDim: config.sunjet.embedDim,
          tenant: config.tenant,
        })
      : null;
  }

  snapshot(): RegistrySnapshot {
    const bridge = this.config.bridge;
    return {
      functions: bridge.getFunctions(),
      states: bridge.getStates(),
      policies: bridge.getPolicies(),
      flows: bridge.getFlows(),
      persona: bridge.getPersona?.() ?? null,
      productBrief: bridge.getProductBrief?.() ?? null,
    };
  }

  /**
   * Recompute the hash from the live registry; on change, rebuild the brief and
   * (fire-and-forget) resync the Sunjet mirror. Cheap when nothing changed —
   * safe to call on every SDK register/unregister AND lazily per turn.
   */
  refresh(): { hash: string; changed: boolean } {
    const snapshot = this.snapshot();
    const hash = registryHash(snapshot);
    if (hash === this.hash) {
      return { hash, changed: false };
    }
    this.hash = hash;
    this.brief = buildCapabilityBrief(snapshot);
    if (this.mirror) {
      // Serialize syncs; a Sunjet outage degrades search to in-process, never a turn.
      const run = async () => {
        try {
          await this.mirror!.sync(snapshot, hash, this.brief);
        } catch (error) {
          console.error(
            '[aelio] Lighthouse mirror sync failed — semantic tool search degrades to in-process:',
            error instanceof Error ? error.message : String(error),
          );
        }
      };
      this.syncing = (this.syncing ?? Promise.resolve()).then(run);
    }
    return { hash, changed: true };
  }

  getHash(): string {
    return this.refresh().hash;
  }

  /**
   * refresh() schedules a Sunjet mirror resync fire-and-forget — correct for
   * callers that don't read the mirror themselves (every SDK register/
   * unregister), but a search that runs immediately after a resync it just
   * triggered would race the mirror's delete-then-reinsert and could see a
   * partially-cleared or empty table, misreporting a real, well-matched
   * capability as unbound. Any caller that's about to read the mirror needs
   * to wait for a sync it just caused to actually finish first.
   */
  private async refreshAndAwaitSync(): Promise<void> {
    const { changed } = this.refresh();
    if (changed && this.syncing) {
      await this.syncing;
    }
  }

  getBrief(): string {
    this.refresh();
    return this.brief;
  }

  /**
   * Ranked semantic tool search. Sunjet mirror (with prerequisite expansion)
   * when available; otherwise an in-process embedding rank over the live
   * registry. Always returns live-registry definitions.
   */
  async searchTools(queryText: string, k: number): Promise<ToolSearchHit[]> {
    await this.refreshAndAwaitSync();
    const registry = this.config.bridge.getFunctions();
    if (this.mirror) {
      try {
        const hits = await this.mirror.searchTools(queryText, k, registry);
        if (hits.length > 0) {
          return hits;
        }
      } catch {
        // fall through to in-process
      }
    }
    return this.searchInProcess(queryText, k, registry);
  }

  /**
   * Feasibility probe for the planner's `refuse` path: fail-open on feasibility.
   * Returns the best capability-similarity score (0 when nothing matches at all).
   */
  async probeFeasibility(queryText: string): Promise<number> {
    await this.refreshAndAwaitSync();
    if (this.mirror) {
      try {
        return await this.mirror.probeCapabilities(queryText);
      } catch {
        // fall through
      }
    }
    const hits = await this.searchInProcess(queryText, 3, this.config.bridge.getFunctions());
    return hits.length > 0 ? (hits[0]?.score ?? 0) : 0;
  }

  private async searchInProcess(
    queryText: string,
    k: number,
    registry: FunctionDefinition[],
  ): Promise<ToolSearchHit[]> {
    if (registry.length === 0) {
      return [];
    }
    const queryVector = await embed(queryText);
    const scored = await Promise.all(
      registry.map(async (fn) => ({
        fn,
        score: cosineSimilarity(queryVector, await embed(toolDescriptor(fn))),
      })),
    );
    scored.sort((a, b) => b.score - a.score);
    return scored.slice(0, k);
  }
}
