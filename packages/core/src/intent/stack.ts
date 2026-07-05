import type { AelioDatabase } from '@aelio/db';
import { sessions } from '@aelio/db';
import { eq } from 'drizzle-orm';
import { randomUUID } from 'node:crypto';

/** One conversational intent frame. Index 0 in the stack = current (top). */
export type IntentFrame = {
  id: string;
  label: string;
  summary: string;
  startedAt: number;
  lastActiveAt: number;
  expiresAt: number;
};

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

export function expireIntentStack(stack: IntentStack, now = Date.now()): IntentStack {
  return stack.filter((frame) => frame.expiresAt > now);
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
  db: AelioDatabase['db'],
  sessionId: string,
): Promise<IntentStack> {
  const row = await db.select().from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  const metadata = (row[0]?.metadata ?? {}) as SessionMetadata;
  return expireIntentStack(metadata.intentStack ?? []);
}

export async function saveIntentStack(
  db: AelioDatabase['db'],
  sessionId: string,
  stack: IntentStack,
): Promise<void> {
  const row = await db.select().from(sessions).where(eq(sessions.id, sessionId)).limit(1);
  const metadata = (row[0]?.metadata ?? {}) as SessionMetadata;
  await db
    .update(sessions)
    .set({
      metadata: {
        ...metadata,
        intentStack: expireIntentStack(stack),
      },
    })
    .where(eq(sessions.id, sessionId));
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

const CONCLUDE_RE =
  /\b(thanks|thank you|that's all|that is all|done|got it|perfect|no more questions|all good|cheers|resolved)\b/i;

const TOPIC_RULES: Array<{ label: string; keywords: RegExp; summary: string }> = [
  {
    label: 'order_inquiry',
    keywords: /\b(order|orders|ship|shipped|tracking|delivery|a-\d{4})\b/i,
    summary: 'User is asking about order status, tracking, or order details.',
  },
  {
    label: 'subscription',
    keywords: /\b(plan|subscription|upgrade|downgrade|renew|billing cycle)\b/i,
    summary: 'User is discussing their subscription plan or billing.',
  },
  {
    label: 'invoice',
    keywords: /\b(invoice|invoices|payment|unpaid|bill)\b/i,
    summary: 'User is asking about invoices or payment status.',
  },
  {
    label: 'cancellation',
    keywords: /\b(cancel|cancellation|refund)\b/i,
    summary: 'User wants to cancel something or get a refund.',
  },
  {
    label: 'greeting',
    keywords: /^(hi|hello|hey|good morning|good afternoon)\b/i,
    summary: 'User is greeting or opening the conversation.',
  },
  {
    label: 'general_support',
    keywords: /./,
    summary: 'General product support conversation.',
  },
];

function classifyIntent(
  userMessage: string,
  assistantReply: string,
  toolNames: string[],
): { label: string; summary: string } {
  if (toolNames.includes('getOrderStatus') || toolNames.includes('listOrders')) {
    return { label: 'order_inquiry', summary: 'User is asking about orders.' };
  }
  if (toolNames.includes('getSubscription') || toolNames.includes('upgradePlan')) {
    return { label: 'subscription', summary: 'User is discussing subscription or plan changes.' };
  }
  if (toolNames.includes('listInvoices')) {
    return { label: 'invoice', summary: 'User is asking about invoices.' };
  }
  if (toolNames.includes('cancelOrder')) {
    return { label: 'cancellation', summary: 'User wants to cancel an order.' };
  }

  const text = `${userMessage} ${assistantReply}`;
  for (const rule of TOPIC_RULES) {
    if (rule.label === 'general_support') {
      continue;
    }
    if (rule.keywords.test(userMessage) || rule.keywords.test(assistantReply)) {
      return { label: rule.label, summary: rule.summary };
    }
  }

  const orderRule = TOPIC_RULES.find((rule) => rule.label === 'order_inquiry');
  if (orderRule?.keywords.test(text)) {
    return { label: orderRule.label, summary: orderRule.summary };
  }

  return { label: 'general_support', summary: 'General product support conversation.' };
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
  // General support is a weak signal — only shift if top was greeting or general.
  if (
    detected.label === 'general_support' &&
    currentTop.label !== 'greeting' &&
    currentTop.label !== 'general_support'
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

  const detected = classifyIntent(input.userMessage, input.assistantReply, toolNames);
  const top = stack[0];

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