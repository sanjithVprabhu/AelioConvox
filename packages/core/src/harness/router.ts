import type { LLMProvider } from '@aelio/llm';
import type { AelioDatabase } from '@aelio/db';
import type { HarnessConfig, RoutedIntent } from './types.js';
import { recordLlmTokenSpend } from './budget.js';

const ROUTER_SYSTEM = `Extract a clean intent string and category from the user's message.
Respond ONLY with JSON: {"intent": string, "category": string|null}

Categories (pick the best match, or null):
- general: greetings, chitchat, simple questions answerable without tools
- info: factual lookup that needs one read tool
- transaction: purchases, checkout, payments, orders
- checkout: cart, COD, payment completion flows
- order: order status, cancellation, refunds
- multi_step: requests that clearly need several tools in sequence
- account: profile, settings, subscription changes`;

export async function routeIntent(input: {
  llm: LLMProvider;
  model: string;
  userMessage: string;
  database?: AelioDatabase['db'];
  planId?: string;
}): Promise<RoutedIntent> {
  const result = await input.llm.complete({
    model: input.model,
    maxTokens: 300,
    system: ROUTER_SYSTEM,
    messages: [{ role: 'user', content: input.userMessage }],
    tools: [],
    responseFormat: 'json',
    telemetry: { purpose: 'harness_router' },
  });

  if (input.database && input.planId) {
    await recordLlmTokenSpend(input.database, input.planId, result.usage);
  }

  const raw = result.text.trim();
  try {
    const parsed = JSON.parse(raw) as { intent?: unknown; category?: unknown };
    return {
      intent: typeof parsed.intent === 'string' ? parsed.intent : input.userMessage,
      category: typeof parsed.category === 'string' ? parsed.category : null,
    };
  } catch {
    return { intent: input.userMessage, category: null };
  }
}

export function shouldUseHarnessAfterRoute(input: {
  route: RoutedIntent;
  config: HarnessConfig;
  hasActiveFlow: boolean;
}): boolean {
  const category = input.route.category?.toLowerCase() ?? '';
  if (['general', 'chitchat', 'greeting'].includes(category)) {
    return false;
  }
  if (input.config.forceOnActiveFlow && input.hasActiveFlow) {
    return true;
  }
  if (input.config.forceCategories.some((entry) => entry.toLowerCase() === category)) {
    return true;
  }
  if (['transaction', 'checkout', 'order', 'multi_step', 'account'].includes(category)) {
    return true;
  }
  return category.length > 0 && category !== 'info';
}
