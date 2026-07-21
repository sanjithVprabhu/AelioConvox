/**
 * Conversation resolution state — deterministic driver for the proactive daemon.
 *
 * The LLM may recommend a follow-up message, but it never chooses whether contact
 * is allowed. That decision is a score + hard suppressions over this state.
 */

export type ResolutionState =
  | 'active'
  | 'awaiting_user'
  | 'awaiting_system'
  | 'resolved'
  | 'satisfied'
  | 'disengaged'
  | 'do_not_contact';

export type ResolutionSnapshot = {
  state: ResolutionState;
  intentLabel?: string;
  flowId?: string | null;
  reason: string;
  updatedAt: number;
  attemptCount: number;
};

export type ResolutionSignals = {
  pathwayStrategy?: string | null;
  pathwayProactive?: string | null;
  stanceOverall?: string | null;
  pendingConfirmation?: boolean;
  toolSuccess?: boolean;
  flowCompleted?: boolean;
  reflectionOutcome?: 'resolved' | 'unresolved' | 'unclear' | null;
  explicitStop?: boolean;
  current?: ResolutionSnapshot | null;
};

export type ProactiveActivationConfig = {
  version: string;
  decisionThreshold: number;
  weights: {
    unresolvedGoal: number;
    promisedFollowup: number;
    activeFlow: number;
    negativeOutcome: number;
    satisfaction: number;
    disengagement: number;
    agePenalty: number;
    previousAttempt: number;
  };
  /** Soft age half-life for unresolved goals (ms). */
  unresolvedHalfLifeMs: number;
};

export const DEFAULT_ACTIVATION_CONFIG: ProactiveActivationConfig = {
  version: 'proactive-v1',
  decisionThreshold: 0.45,
  weights: {
    unresolvedGoal: 0.35,
    promisedFollowup: 0.2,
    activeFlow: 0.2,
    negativeOutcome: 0.15,
    satisfaction: 0.35,
    disengagement: 0.5,
    agePenalty: 0.15,
    previousAttempt: 0.1,
  },
  unresolvedHalfLifeMs: 3 * 24 * 60 * 60 * 1000,
};

export function deriveResolutionState(signals: ResolutionSignals): ResolutionSnapshot {
  const now = Date.now();
  const attemptCount = signals.current?.attemptCount ?? 0;

  if (signals.explicitStop || signals.pathwayStrategy === 'disengage') {
    return {
      state: 'do_not_contact',
      intentLabel: signals.current?.intentLabel,
      flowId: signals.current?.flowId,
      reason: 'User signalled closure or stop',
      updatedAt: now,
      attemptCount,
    };
  }

  if (signals.stanceOverall === 'negative' && signals.pathwayProactive === 'suppress') {
    return {
      state: 'disengaged',
      reason: 'Negative stance with proactive suppress',
      updatedAt: now,
      attemptCount,
    };
  }

  if (signals.pendingConfirmation) {
    return {
      state: 'awaiting_user',
      reason: 'Pending confirmation',
      updatedAt: now,
      attemptCount,
    };
  }

  if (signals.flowCompleted || signals.reflectionOutcome === 'resolved') {
    return {
      state: 'resolved',
      reason: signals.flowCompleted ? 'Flow completed' : 'Reflection marked resolved',
      updatedAt: now,
      attemptCount,
    };
  }

  if (signals.toolSuccess && signals.stanceOverall === 'positive') {
    return {
      state: 'satisfied',
      reason: 'Successful tool outcome with positive stance',
      updatedAt: now,
      attemptCount,
    };
  }

  if (signals.reflectionOutcome === 'unresolved' || signals.pathwayProactive === 'consider_followup') {
    return {
      state: 'awaiting_user',
      reason: 'Unresolved goal / consider follow-up',
      updatedAt: now,
      attemptCount,
    };
  }

  return {
    state: signals.current?.state ?? 'active',
    intentLabel: signals.current?.intentLabel,
    flowId: signals.current?.flowId,
    reason: signals.current?.reason ?? 'No stronger signal; keep active',
    updatedAt: now,
    attemptCount,
  };
}

export type ActivationDecision = {
  action: 'none' | 'wait' | 'nudge';
  score: number;
  reason: string;
  suppressed: boolean;
};

const HARD_SUPPRESS: ResolutionState[] = ['resolved', 'satisfied', 'disengaged', 'do_not_contact'];

export function scoreProactiveActivation(input: {
  snapshot: ResolutionSnapshot;
  now?: number;
  config?: ProactiveActivationConfig;
}): ActivationDecision {
  const cfg = input.config ?? DEFAULT_ACTIVATION_CONFIG;
  const now = input.now ?? Date.now();
  const snap = input.snapshot;

  if (HARD_SUPPRESS.includes(snap.state)) {
    return {
      action: 'none',
      score: 0,
      reason: `hard suppress: ${snap.state}`,
      suppressed: true,
    };
  }

  const ageMs = Math.max(0, now - snap.updatedAt);
  const ageFactor = Math.exp((-Math.LN2 * ageMs) / cfg.unresolvedHalfLifeMs);
  const unresolved =
    snap.state === 'awaiting_user' || snap.state === 'awaiting_system' || snap.state === 'active'
      ? 1
      : 0;
  const promised = snap.state === 'awaiting_user' ? 1 : 0;
  const activeFlow = snap.flowId ? 1 : 0;
  const negative = /frustrat|negative|unresolved/i.test(snap.reason) ? 1 : 0;
  const attempts = Math.min(1, snap.attemptCount / 3);

  const score = Math.max(
    0,
    Math.min(
      1,
      cfg.weights.unresolvedGoal * unresolved +
        cfg.weights.promisedFollowup * promised +
        cfg.weights.activeFlow * activeFlow +
        cfg.weights.negativeOutcome * negative -
        cfg.weights.agePenalty * (1 - ageFactor) -
        cfg.weights.previousAttempt * attempts,
    ),
  );

  if (score < cfg.decisionThreshold) {
    return {
      action: 'wait',
      score,
      reason: `below threshold (${score.toFixed(2)} < ${cfg.decisionThreshold})`,
      suppressed: false,
    };
  }

  return {
    action: 'nudge',
    score,
    reason: `activation score ${score.toFixed(2)} — ${snap.reason}`,
    suppressed: false,
  };
}
