// Provider wiring test — proves the three first-class chat providers (Anthropic,
// OpenAI, Gemini) and their embedding providers are constructible from config,
// that provider selection is purely config-driven, and that a missing key fails
// loudly. No network calls. Run: node scripts/test-llm-providers.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import {
  createLLMProvider,
  createLLMProviderChain,
  createEmbeddingProvider,
  SUPPORTED_LLM_PROVIDERS,
} from '@aelio/llm';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
let failures = 0;
const ok = (cond, msg) => {
  if (cond) console.log('  ✓', msg);
  else {
    failures += 1;
    console.error('  ✗', msg);
  }
};

console.log('\n[1] Each first-class chat provider constructs from config (no network)');
for (const provider of ['anthropic', 'openai', 'gemini']) {
  const llm = createLLMProvider({ provider, model: 'm', apiKey: 'test-key' });
  ok(typeof llm.complete === 'function', `${provider} → provider with .complete()`);
}
ok(SUPPORTED_LLM_PROVIDERS.includes('anthropic') &&
   SUPPORTED_LLM_PROVIDERS.includes('openai') &&
   SUPPORTED_LLM_PROVIDERS.includes('gemini'),
   'all three advertised in SUPPORTED_LLM_PROVIDERS');

console.log('\n[2] Missing api_key fails loudly with a provider-named message');
for (const provider of ['anthropic', 'openai', 'gemini']) {
  let threw = null;
  try { createLLMProvider({ provider, model: 'm' }); } catch (e) { threw = e; }
  ok(threw && /api_key/i.test(threw.message), `${provider} without key throws (${threw?.message?.slice(0, 48)}…)`);
}

console.log('\n[3] Embedding providers construct for all three');
for (const [provider, model] of [['anthropic', 'voyage-3-lite'], ['openai', 'text-embedding-3-small'], ['gemini', 'text-embedding-004']]) {
  const emb = createEmbeddingProvider({ provider, model, apiKey: 'test-key' });
  ok(typeof emb.embed === 'function', `${provider} embeddings → provider with .embed()`);
}

console.log('\n[4] Fallback chain: primary + fallback compose');
{
  const chain = createLLMProviderChain([
    { provider: 'anthropic', model: 'claude', apiKey: 'k1' },
    { provider: 'openai', model: 'gpt', apiKey: 'k2' },
  ]);
  ok(typeof chain.complete === 'function', 'anthropic→openai fallback chain built');
}

console.log('\n[5] Shipped config.{provider}.yaml selects the matching provider + key ref');
for (const provider of ['anthropic', 'openai', 'gemini']) {
  const text = readFileSync(join(root, `config.${provider}.yaml`), 'utf8');
  ok(new RegExp(`provider:\\s*${provider}`).test(text), `config.${provider}.yaml sets llm.provider: ${provider}`);
  ok(/api_key:\s*\$\{[A-Z_]+\}/.test(text), `config.${provider}.yaml references an api_key env var`);
}

if (failures > 0) {
  console.error(`\n${failures} provider-wiring assertion(s) failed`);
  process.exit(1);
}
console.log('\nAll LLM provider-wiring tests passed.');
