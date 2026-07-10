import type { LLMCompleteOptions, LLMCompleteResult, LLMProvider } from '../types.js';

/**
 * Test-only provider that replays a scripted queue of results in order.
 * Unlike MockProvider (keyword heuristics), this gives harness tests exact
 * control over planner/binder/synthesis outputs — including malformed ones,
 * to exercise validation and retry paths. Never wired into the provider
 * factory; construct it directly in test scripts.
 */
export class ScriptedProvider implements LLMProvider {
  private readonly queue: LLMCompleteResult[];
  /** Every call's options, recorded for assertions (prompt content, toolChoice…). */
  readonly calls: LLMCompleteOptions[] = [];

  constructor(script: LLMCompleteResult[]) {
    this.queue = [...script];
  }

  async complete(opts: LLMCompleteOptions): Promise<LLMCompleteResult> {
    this.calls.push(opts);
    const next = this.queue.shift();
    if (!next) {
      throw new Error(
        `ScriptedProvider exhausted after ${this.calls.length} call(s) — script too short for this flow`,
      );
    }
    return next;
  }

  get remaining(): number {
    return this.queue.length;
  }
}
