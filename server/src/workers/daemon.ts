import {
  deriveResolutionState,
  findUnreflectedSessions,
  reflectOnSession,
  scoreProactiveActivation,
  sendProactiveMessage,
  type ResolutionSnapshot,
} from '@aelio/core';
import type { Channel } from '@aelio/protocol';
import type { RuntimeDeps } from '../runtime-deps.js';

/**
 * The reflection + proactive daemon. Reviews finished sessions, updates
 * resolution state, and may send a follow-up only when the deterministic
 * activation scorer says so. Off unless `daemon.enabled` is set.
 */
async function maybeFollowUp(
  deps: RuntimeDeps,
  sessionId: string,
  customerId: string,
  content: string,
  reflectionOutcome: 'resolved' | 'unresolved' | 'unclear',
): Promise<void> {
  const [customer, session] = await Promise.all([
    deps.customerStore.getById(customerId),
    deps.sessionStore.get(sessionId),
  ]);
  const externalId = customer?.externalId;
  const channel = session?.channel;

  if (!externalId || !channel || !session) {
    return;
  }

  const pathway = session.metadata?.semanticPathway as
    | { proactive?: { action?: string; reason?: string }; strategy?: string }
    | undefined;
  const stance = session.metadata?.conversationalStance as
    | { overall?: { valence?: string } }
    | undefined;
  const prior = session.metadata?.resolution as ResolutionSnapshot | undefined;

  const snapshot = deriveResolutionState({
    pathwayStrategy: pathway?.strategy ?? null,
    pathwayProactive: pathway?.proactive?.action ?? null,
    stanceOverall: stance?.overall?.valence ?? null,
    reflectionOutcome,
    explicitStop: pathway?.proactive?.action === 'suppress',
    current: prior ?? null,
  });

  await deps.sessionStore.updateMetadata(sessionId, { resolution: snapshot });

  const activation = scoreProactiveActivation({ snapshot });
  if (activation.suppressed || activation.action !== 'nudge') {
    console.log(
      `[daemon] follow-up skipped for session ${sessionId}: ${activation.reason}`,
    );
    deps.tracer?.trace({
      turnId: `proactive:${sessionId}`,
      sessionId,
      kind: 'proactive',
      payload: {
        source: 'reflection_daemon',
        decision: activation.suppressed ? 'suppressed' : 'wait',
        reason: activation.reason,
        score: activation.score,
        resolution: snapshot.state,
      },
    });
    return;
  }

  // Legacy pathway hint still gates when present and not consider_followup.
  if (pathway?.proactive?.action && pathway.proactive.action !== 'consider_followup') {
    console.log(
      `[daemon] follow-up suppressed for session ${sessionId}: ${pathway.proactive.reason ?? pathway.proactive.action}`,
    );
    deps.tracer?.trace({
      turnId: `proactive:${sessionId}`,
      sessionId,
      kind: 'proactive',
      payload: {
        source: 'reflection_daemon',
        decision: 'suppressed',
        reason: pathway.proactive.reason ?? pathway.proactive.action,
      },
    });
    return;
  }

  const result = await sendProactiveMessage({
    config: {
      enabled: deps.config.proactive.enabled,
      requireOptIn: deps.config.proactive.require_opt_in,
      maxPerCustomerPerDay: deps.config.proactive.max_per_customer_per_day,
      windowHours: deps.config.proactive.window_hours,
    },
    customerExternalId: externalId,
    channel: channel as Channel,
    content,
    dedupKey: `followup:${sessionId}`,
    customerStore: deps.customerStore,
    messageStore: deps.messageStore,
    proactiveStore: deps.proactiveStore,
    jobStore: deps.jobStore,
  });

  if (result.status === 'sent') {
    await deps.sessionStore.updateMetadata(sessionId, {
      resolution: {
        ...snapshot,
        attemptCount: (snapshot.attemptCount ?? 0) + 1,
        updatedAt: Date.now(),
        reason: `nudge sent: ${activation.reason}`,
      },
    });
  }

  console.log(
    `[daemon] follow-up for session ${sessionId}: ${result.status}${result.reason ? ` (${result.reason})` : ''}`,
  );
  deps.tracer?.trace({
    turnId: `proactive:${sessionId}`,
    sessionId,
    kind: 'proactive',
    payload: {
      source: 'reflection_daemon',
      decision: result.status === 'sent' ? 'nudge_sent' : 'blocked',
      reason: result.reason ?? activation.reason,
      score: activation.score,
      resolution: snapshot.state,
      content,
      dedupKey: `followup:${sessionId}`,
    },
  });
}

export function startDaemonWorker(deps: RuntimeDeps) {
  const cfg = deps.config.daemon;
  if (!cfg.enabled) {
    return () => {};
  }

  let running = false;

  const runCycle = async () => {
    if (running) return;
    running = true;
    try {
      const sessions = await findUnreflectedSessions(
        cfg.max_per_cycle,
        cfg.reflect_min_messages,
        deps.sessionStore,
        deps.reflectionStore,
        deps.messageStore,
      );
      for (const { sessionId, customerId } of sessions) {
        try {
          const reflection = await reflectOnSession({
            llm: deps.llm,
            model: deps.config.llm.model,
            maxTokens: deps.config.llm.max_tokens,
            sessionId,
            customerId,
            memoryStore: deps.memoryStore,
            messageStore: deps.messageStore,
            functionCallStore: deps.functionCallStore,
            reflectionStore: deps.reflectionStore,
          });
          if (reflection) {
            console.log(
              `[daemon] reflected on session ${sessionId}: ${reflection.outcome} (score ${reflection.score})`,
            );

            if (
              cfg.proactive_followup &&
              deps.config.proactive.enabled &&
              reflection.outcome === 'unresolved' &&
              reflection.followup
            ) {
              await maybeFollowUp(
                deps,
                sessionId,
                customerId,
                reflection.followup,
                reflection.outcome,
              );
            }
          }
        } catch (error) {
          console.error(
            `[daemon] reflection failed for session ${sessionId}:`,
            error instanceof Error ? error.message : String(error),
          );
        }
      }
    } finally {
      running = false;
    }
  };

  const interval = setInterval(() => {
    void runCycle();
  }, cfg.interval_minutes * 60 * 1000);

  interval.unref();
  return () => clearInterval(interval);
}
