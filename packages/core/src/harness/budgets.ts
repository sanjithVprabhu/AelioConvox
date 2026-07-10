import type { HarnessBudgets } from './schema.js';

export type BudgetCheck = { ok: true } | { ok: false; reason: string };

/**
 * Deterministic loop protection. The LLM never controls harness exit — these
 * counters do. Every limit that trips is surfaced as a reason string that the
 * synthesis step can apologize with and telemetry can count.
 */
export class BudgetMeter {
  private toolCalls = 0;
  private replans = 0;
  private tokens = 0;
  private readonly startedAt: number;
  private readonly seenCalls = new Map<string, number>();

  constructor(
    private readonly budgets: HarnessBudgets,
    now = Date.now(),
  ) {
    this.startedAt = now;
  }

  checkPlanSize(instructionCount: number): BudgetCheck {
    if (instructionCount > this.budgets.maxInstructions) {
      return {
        ok: false,
        reason: `plan has ${instructionCount} steps (max ${this.budgets.maxInstructions})`,
      };
    }
    return { ok: true };
  }

  /**
   * Record an outgoing tool call. Fails on budget exhaustion and on progress
   * stall — the same (tool, argsHash) invoked a third time is a loop, not work.
   */
  noteToolCall(toolName: string, argsHash: string): BudgetCheck {
    this.toolCalls += 1;
    if (this.toolCalls > this.budgets.maxToolCalls) {
      return { ok: false, reason: `tool-call budget exhausted (${this.budgets.maxToolCalls})` };
    }
    const key = `${toolName}:${argsHash}`;
    const seen = (this.seenCalls.get(key) ?? 0) + 1;
    this.seenCalls.set(key, seen);
    if (seen >= 3) {
      return { ok: false, reason: `no progress: ${toolName} repeated ${seen}x with identical args` };
    }
    return this.checkClock();
  }

  noteReplan(): BudgetCheck {
    this.replans += 1;
    if (this.replans > this.budgets.maxReplans) {
      return { ok: false, reason: `replan budget exhausted (${this.budgets.maxReplans})` };
    }
    return this.checkClock();
  }

  noteUsage(usage?: { inputTokens: number; outputTokens: number }): BudgetCheck {
    if (usage) {
      this.tokens += usage.inputTokens + usage.outputTokens;
      if (this.tokens > this.budgets.maxTurnTokens) {
        return { ok: false, reason: `token budget exhausted (${this.budgets.maxTurnTokens})` };
      }
    }
    return { ok: true };
  }

  checkClock(now = Date.now()): BudgetCheck {
    if (now - this.startedAt > this.budgets.wallClockMs) {
      return { ok: false, reason: `wall-clock budget exhausted (${this.budgets.wallClockMs}ms)` };
    }
    return { ok: true };
  }

  snapshot(): { toolCalls: number; replans: number; tokens: number; elapsedMs: number } {
    return {
      toolCalls: this.toolCalls,
      replans: this.replans,
      tokens: this.tokens,
      elapsedMs: Date.now() - this.startedAt,
    };
  }
}
