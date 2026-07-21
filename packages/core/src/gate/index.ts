/**
 * Generic hot-path gate — deterministic template replies for contentless
 * messages (bare greetings, thanks, farewells). Runs BEFORE the semantic
 * response cache and the retrieval fan-out, so a "hi" costs zero embedding
 * lookups and zero LLM tokens and returns in microseconds.
 *
 * Deliberately narrow: only full-match, short messages qualify. Anything that
 * could be an answer to a question the assistant just asked (yes/no/ok) is
 * NOT gated — those must reach the model with conversation context. The gate
 * is also skipped entirely while a harness plan is parked awaiting input or a
 * recoil intent is collecting a value, where even a "thanks" may carry meaning.
 */

export type GenericKind = 'greeting' | 'thanks' | 'farewell';

export type GenericGateResult =
  | { hit: false }
  | { hit: true; kind: GenericKind; reply: string; normalized: string };

export type GenericGateOptions = {
  /** Override the template reply per kind (e.g. tenant voice). */
  replies?: Partial<Record<GenericKind, string>>;
};

const DEFAULT_REPLIES: Record<GenericKind, string> = {
  greeting: 'Hi! How can I help you today?',
  thanks: "You're welcome! Is there anything else I can help with?",
  farewell: 'Goodbye! Feel free to reach out anytime.',
};

/** Full-match patterns over the normalized message. Order = first match wins. */
const RULES: Array<{ kind: GenericKind; pattern: RegExp }> = [
  {
    kind: 'greeting',
    pattern:
      /^(hi+|hello+|hey+|heya|yo|howdy|good (morning|afternoon|evening)|(hi|hello|hey) there)$/,
  },
  {
    kind: 'thanks',
    pattern:
      /^(thanks|thank you|thanks a lot|thanks so much|thank you so much|many thanks|thx|ty|tysm|(great|perfect|awesome|cool)[ ,]*(thanks|thank you))$/,
  },
  {
    kind: 'farewell',
    pattern:
      /^(bye+|goodbye|bye bye|see (you|ya)( later)?|good night|gn|(ok(ay)? )?(that s|thats) all( for now)?( thanks| thank you)?)$/,
  },
];

/** Words beyond this after normalization disqualify the message — it has content. */
const MAX_GATE_WORDS = 5;

function normalize(message: string): string {
  return message
    .toLowerCase()
    .replace(/[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{FE0F}]/gu, ' ')
    .replace(/[^a-z ]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export function evaluateGenericGate(
  message: string,
  options?: GenericGateOptions,
): GenericGateResult {
  const normalized = normalize(message);
  if (!normalized || normalized.split(' ').length > MAX_GATE_WORDS) {
    return { hit: false };
  }
  for (const rule of RULES) {
    if (rule.pattern.test(normalized)) {
      return {
        hit: true,
        kind: rule.kind,
        reply: options?.replies?.[rule.kind] ?? DEFAULT_REPLIES[rule.kind],
        normalized,
      };
    }
  }
  return { hit: false };
}
