import type { FunctionDefinition } from '@aelio/protocol';
import { randomUUID } from 'node:crypto';
import type { ConvoxSessionStore } from '../storage/sessions.js';

/** One conversational intent frame. Index 0 in the stack = current (top). */
export type IntentFrame = {
  id: string;
  label: string;
  summary: string;
  startedAt: number;
  lastActiveAt: number;
  expiresAt: number;
  /**
   * 'user' = a topic the customer raised; 'recoil' = a system sub-intent that
   * exists only to collect a value the harness needs. Optional for backward
   * compatibility with frames persisted before v2 (treated as 'user').
   */
  kind?: 'user' | 'recoil';
};

/** How many top frames form the always-alive spine (never TTL-evicted). */
const SPINE_DEPTH = 3;

export type IntentStack = IntentFrame[];

export type IntentStackConfig = {
  enabled: boolean;
  ttlMinutes: number;
  maxDepth: number;
};

type SessionMetadata = {
  intentStack?: IntentStack;
  [key: string]: unknown;
};

/**
 * Expire stale frames — but never evict the spine (the top {@link SPINE_DEPTH}
 * frames). The root goal (order refund) must survive even when detours and
 * clarifications pile on top of it; only non-spine frames time out. This fixes
 * the positional-eviction bug where a buried-but-active goal would vanish.
 */
export function expireIntentStack(stack: IntentStack, now = Date.now()): IntentStack {
  return stack.filter((frame, index) => index < SPINE_DEPTH || frame.expiresAt > now);
}

export function buildIntentStackPrompt(stack: IntentStack): string {
  const active = expireIntentStack(stack);
  if (active.length === 0) {
    return '';
  }

  const lines = active.map((frame, index) => {
    const role = index === 0 ? 'CURRENT (focus here)' : `background layer ${index}`;
    return `${role} — ${frame.label}: ${frame.summary}`;
  });

  return [
    'Conversation intent stack (top = current focus; deeper layers are prior topics still in context until they expire or are concluded):',
    ...lines,
    'Stay focused on the CURRENT intent unless the user clearly shifts topic. Prior layers provide background only — do not pursue their goals unless the user returns to them.',
  ].join('\n');
}

export async function loadIntentStack(
  sessionId: string,
  sessionStore: ConvoxSessionStore,
): Promise<IntentStack> {
  if (!sessionStore) {
    throw new Error('Sunjet sessionStore is required');
  }
  const record = await sessionStore.get(sessionId);
  const metadata = (record?.metadata ?? {}) as SessionMetadata;
  return expireIntentStack(metadata.intentStack ?? []);
}

export async function saveIntentStack(
  sessionId: string,
  stack: IntentStack,
  sessionStore: ConvoxSessionStore,
): Promise<void> {
  if (!sessionStore) {
    throw new Error('Sunjet sessionStore is required');
  }
  const record = await sessionStore.get(sessionId);
  const metadata = (record?.metadata ?? {}) as SessionMetadata;
  await sessionStore.updateSummary(sessionId, record?.summary ?? '', {
    ...metadata,
    intentStack: expireIntentStack(stack),
  });
}

function ttlMs(config: IntentStackConfig): number {
  return config.ttlMinutes * 60_000;
}

function pushIntent(
  stack: IntentStack,
  input: { label: string; summary: string },
  config: IntentStackConfig,
  now = Date.now(),
): IntentStack {
  const frame: IntentFrame = {
    id: randomUUID(),
    label: input.label,
    summary: input.summary,
    startedAt: now,
    lastActiveAt: now,
    expiresAt: now + ttlMs(config),
  };
  const next = [frame, ...expireIntentStack(stack, now)];
  return next.slice(0, config.maxDepth);
}

function refreshTop(stack: IntentStack, config: IntentStackConfig, now = Date.now()): IntentStack {
  const top = stack[0];
  if (!top) {
    return stack;
  }
  return [
    {
      ...top,
      lastActiveAt: now,
      expiresAt: now + ttlMs(config),
    },
    ...stack.slice(1),
  ];
}

function popTop(stack: IntentStack): IntentStack {
  return stack.slice(1);
}

// Only two UNIVERSAL linguistic signals live in core: greetings and conversation
// conclusions. Every domain topic comes from the tenant via tool `intent` labels
// declared in the SDK — the core never hardcodes a business vocabulary.
const CONCLUDE_RE =
  /\b(thanks|thank you|that's all|that is all|done|got it|perfect|no more questions|all good|cheers|resolved)\b/i;

const GREETING_RE = /^(hi|hello|hey|good morning|good afternoon|good evening)\b/i;

export type IntentSource = Pick<FunctionDefinition, 'name' | 'description' | 'intent'>;

function toolIntent(tool: IntentSource): { label: string; summary: string } {
  const label = tool.intent ?? tool.name;
  return { label, summary: `User is engaged with: ${tool.description}` };
}

/**
 * Classify the turn's intent from what actually happened, in the tenant's own
 * vocabulary:
 *   1. A tool executed → that tool's declared `intent` (fallback: its name).
 *   2. No tool → greeting detection, else keep the current topic (`null` = no
 *      opinion; the stack refreshes the top frame instead of guessing).
 */
function classifyIntent(
  userMessage: string,
  toolNames: string[],
  registeredTools: IntentSource[],
): { label: string; summary: string } | null {
  for (const name of toolNames) {
    const tool = registeredTools.find((entry) => entry.name === name);
    if (tool) {
      return toolIntent(tool);
    }
  }

  if (GREETING_RE.test(userMessage.trim())) {
    return { label: 'greeting', summary: 'User is greeting or opening the conversation.' };
  }

  return null;
}

function isConclusion(userMessage: string): boolean {
  return CONCLUDE_RE.test(userMessage.trim());
}

function isTopicShift(
  currentTop: IntentFrame | undefined,
  detected: { label: string; summary: string },
): boolean {
  if (!currentTop) {
    return true;
  }
  if (currentTop.label === detected.label) {
    return false;
  }
  // Greeting on top of a real topic is not a shift — refresh instead.
  if (detected.label === 'greeting' && currentTop.label !== 'greeting') {
    return false;
  }
  // "general" is a weak signal — never displace a concrete tenant intent with it.
  if (
    detected.label === 'general' &&
    currentTop.label !== 'greeting' &&
    currentTop.label !== 'general'
  ) {
    return false;
  }
  return true;
}

/**
 * Update the intent stack after a turn completes.
 * - Same intent → refresh TTL on top frame
 * - New intent → push on top (stack), prior intents sink deeper
 * - Conclusion → pop top frame
 * - Expired frames → removed automatically
 */
export function updateIntentStack(input: {
  stack: IntentStack;
  userMessage: string;
  assistantReply: string;
  toolNames?: string[];
  registeredTools?: IntentSource[];
  /** Pre-turn semantic classification; used when no executed tool provides a stronger signal. */
  semanticIntent?: { label: string; confidence: number };
  config: IntentStackConfig;
  now?: number;
}): IntentStack {
  if (!input.config.enabled) {
    return input.stack;
  }

  const now = input.now ?? Date.now();
  let stack = expireIntentStack(input.stack, now);
  const toolNames = input.toolNames ?? [];

  if (isConclusion(input.userMessage) && stack.length > 0) {
    stack = popTop(stack);
    return stack;
  }

  const toolDetected = classifyIntent(
    input.userMessage,
    toolNames,
    input.registeredTools ?? [],
  );
  const detected =
    toolDetected ??
    (input.semanticIntent && input.semanticIntent.confidence >= 0.12
      ? {
          label: input.semanticIntent.label,
          summary: `User's current request is semantically aligned with: ${input.semanticIntent.label}.`,
        }
      : null);
  const top = stack[0];

  // No signal this turn: keep the current focus alive rather than guessing.
  if (!detected) {
    if (!top) {
      return pushIntent(
        stack,
        { label: 'general', summary: 'General conversation with the customer.' },
        input.config,
        now,
      );
    }
    return refreshTop(stack, input.config, now);
  }

  if (!top) {
    return pushIntent(stack, detected, input.config, now);
  }

  if (isTopicShift(top, detected)) {
    return pushIntent(stack, detected, input.config, now);
  }

  const refreshed = refreshTop(stack, input.config, now);
  if (refreshed[0] && detected.summary !== top.summary) {
    refreshed[0] = { ...refreshed[0], summary: detected.summary };
  }
  return refreshed;
}