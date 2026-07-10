import { findUnreflectedSessions, reflectOnSession, sendProactiveMessage } from '@aelio/core';
import type { RuntimeDeps } from '../runtime-deps.js';

/**
 * The reflection daemon — a scheduled, budgeted, idempotent harness that reviews
 * finished sessions with the LLM and feeds insights back into memory. Off unless
 * `daemon.enabled` is set.
 */
async function maybeFollowUp(
  deps: RuntimeDeps,
  sessionId: string,
  customerId: string,
  content: string,
): Promise<void> {
  const customer = deps.database.sqlite
    .prepare('SELECT external_id AS externalId FROM customers WHERE id = ?')
    .get(customerId) as { externalId: string | null } | undefined;
  const session = deps.database.sqlite
    .prepare('SELECT channel FROM sessions WHERE id = ?')
    .get(sessionId) as { channel: string } | undefined;

  if (!customer?.externalId || !session?.channel) {
    return;
  }

  const result = await sendProactiveMessage({
    database: deps.database,
    config: {
      enabled: deps.config.proactive.enabled,
      requireOptIn: deps.config.proactive.require_opt_in,
      maxPerCustomerPerDay: deps.config.proactive.max_per_customer_per_day,
      windowHours: deps.config.proactive.window_hours,
    },
    customerExternalId: customer.externalId,
    channel: session.channel,
    content,
    dedupKey: `followup:${sessionId}`, // one autonomous follow-up per session, ever
  });

  console.log(
    `[daemon] follow-up for session ${sessionId}: ${result.status}${result.reason ? ` (${result.reason})` : ''}`,
  );
}

export function startDaemonWorker(deps: RuntimeDeps) {
  const cfg = deps.config.daemon;
  if (!cfg.enabled) {
    return () => {};
  }

  let running = false;

  const runCycle = async () => {
    if (running) return; // never overlap cycles
    running = true;
    try {
      const sessions = findUnreflectedSessions(
        deps.database,
        cfg.max_per_cycle,
        cfg.reflect_min_messages,
      );
      for (const { sessionId, customerId } of sessions) {
        try {
          const reflection = await reflectOnSession({
            database: deps.database,
            llm: deps.llm,
            model: deps.config.llm.background_model ?? deps.config.llm.model,
            maxTokens: deps.config.llm.max_tokens,
            sessionId,
            customerId,
          });
          if (reflection) {
            console.log(
              `[daemon] reflected on session ${sessionId}: ${reflection.outcome} (score ${reflection.score})`,
            );

            // Autonomous follow-up: an unresolved session can trigger a proactive
            // re-engagement — but only if enabled, and still subject to EVERY
            // proactive guardrail (opt-in, dedup, frequency cap, 24h window).
            if (
              cfg.proactive_followup &&
              deps.config.proactive.enabled &&
              reflection.outcome === 'unresolved' &&
              reflection.followup
            ) {
              await maybeFollowUp(deps, sessionId, customerId, reflection.followup);
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
