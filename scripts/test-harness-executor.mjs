// Harness executor unit tests — resolver DAG, wavefront parallelism, write
// sequencing, gates, and idempotency. No server/LLM: drives the executor
// directly with a fake SDK. Run: node scripts/test-harness-executor.mjs
import {
  resolvePlan,
  executePlan,
  newExecutorState,
  evaluateGate,
} from '@aelio/core';

let failures = 0;
function assert(cond, msg) {
  if (!cond) {
    failures += 1;
    console.error('  ✗', msg);
  } else {
    console.log('  ✓', msg);
  }
}

const readTool = (name, params = {}) => ({ name, description: name, params, safety: 'read' });
const writeTool = (name, params = {}) => ({ name, description: name, params, safety: 'write' });

// A fake SDK that records call order/concurrency and returns scripted data.
function fakeSdk(handlers) {
  const calls = [];
  let active = 0;
  let maxConcurrent = 0;
  return {
    calls,
    getMaxConcurrent: () => maxConcurrent,
    async invoke(name, args) {
      active += 1;
      maxConcurrent = Math.max(maxConcurrent, active);
      calls.push({ name, args });
      await new Promise((r) => setTimeout(r, 10));
      active -= 1;
      const data = handlers[name] ? handlers[name](args) : { ok: true };
      return { ok: true, data, durationMs: 1 };
    },
  };
}

const baseSafety = { defaultMode: 'full', requireConfirmationFor: [] };
const ctx = { customerId: 'c', sessionId: 's', channel: 'web', channelAddress: 'web:c' };
const { BudgetMeter } = await import('@aelio/core');
const budgets = () => new BudgetMeter({
  maxInstructions: 20, maxReplans: 2, maxRecoilsPerIntent: 3,
  maxToolCalls: 50, wallClockMs: 60000, maxTurnTokens: 100000,
});

// ---------------------------------------------------------------------------
console.log('\n[1] Spine bug: apply_coupon(cart_id) depends on add_to_cart → sequenced');
{
  const bound = [
    { instruction: { id: 'a', capability: 'add to cart', produces: ['cart_id'] }, tool: writeTool('add_to_cart') },
    { instruction: { id: 'b', capability: 'apply coupon' }, tool: writeTool('apply_coupon', { cart_id: 'string' }) },
  ];
  const res = resolvePlan('checkout', undefined, bound);
  assert(res.ok, 'plan resolves');
  const b = res.plan.instructions.find((i) => i.id === 'b');
  assert(b.needs.includes('a'), 'b depends on a (derived from cart_id, not declared)');
  assert(b.argSources.cart_id?.kind === 'output', 'cart_id sourced from a output');

  const sdk = fakeSdk({ add_to_cart: () => ({ cart_id: 'CART-1' }) });
  const state = newExecutorState();
  const outcome = await executePlan(res.plan, state, { sdk, context: ctx, safety: baseSafety, budgets: budgets() });
  assert(outcome.kind === 'complete', 'executes to completion');
  assert(sdk.calls[0].name === 'add_to_cart' && sdk.calls[1].name === 'apply_coupon', 'ran in dependency order');
  assert(sdk.calls[1].args.cart_id === 'CART-1', 'cart_id piped from add_to_cart output into apply_coupon');
}

// ---------------------------------------------------------------------------
console.log('\n[2] Rivers: independent reads run in parallel');
{
  const bound = [
    { instruction: { id: 'a', capability: 'get orders' }, tool: readTool('list_orders') },
    { instruction: { id: 'b', capability: 'get profile' }, tool: readTool('get_profile') },
    { instruction: { id: 'c', capability: 'get invoices' }, tool: readTool('list_invoices') },
  ];
  const res = resolvePlan('overview', undefined, bound);
  const sdk = fakeSdk({});
  const state = newExecutorState();
  await executePlan(res.plan, state, { sdk, context: ctx, safety: baseSafety, budgets: budgets() });
  assert(sdk.getMaxConcurrent() === 3, 'all 3 independent reads ran concurrently');
}

// ---------------------------------------------------------------------------
console.log('\n[3] Writes never race: two independent writes still sequence');
{
  const bound = [
    { instruction: { id: 'a', capability: 'w1' }, tool: writeTool('w1') },
    { instruction: { id: 'b', capability: 'w2' }, tool: writeTool('w2') },
  ];
  const res = resolvePlan('two writes', undefined, bound);
  const sdk = fakeSdk({});
  const state = newExecutorState();
  await executePlan(res.plan, state, { sdk, context: ctx, safety: baseSafety, budgets: budgets() });
  assert(sdk.getMaxConcurrent() === 1, 'writes ran one at a time');
}

// ---------------------------------------------------------------------------
console.log('\n[4] Gate: destructive tool denied fatally');
{
  const v = evaluateGate(
    { name: 'wipe', description: 'wipe', params: {}, safety: 'destructive' },
    {},
    { safety: baseSafety },
  );
  assert(v.verdict === 'deny_fatal', 'destructive → deny_fatal');
}

console.log('\n[5] Gate: write needing confirmation → needs_approval');
{
  const v = evaluateGate(
    writeTool('cancel'),
    {},
    { safety: { defaultMode: 'full', requireConfirmationFor: ['write'] } },
  );
  assert(v.verdict === 'needs_approval', 'write + requireConfirmationFor → needs_approval');
}

console.log('\n[6] Gate: missing required arg → needs_info');
{
  const v = evaluateGate(readTool('lookup', { id: 'string' }), {}, { safety: baseSafety });
  assert(v.verdict === 'needs_info' && v.missing.includes('id'), 'missing id → needs_info');
}

console.log('\n[7] Idempotency: a completed call is not re-invoked on a second executePlan pass');
{
  const bound = [{ instruction: { id: 'a', capability: 'charge', args_hint: { amount: 10 } }, tool: writeTool('charge', { amount: 'number' }) }];
  const res = resolvePlan('charge', undefined, bound);
  const sdk = fakeSdk({});
  const state = newExecutorState();
  await executePlan(res.plan, state, { sdk, context: ctx, safety: baseSafety, budgets: budgets() });
  // Re-run with the same ledger/state: the recorded ledger entry must short-circuit.
  state.completed.delete('a'); // simulate a resume re-entering the wave
  await executePlan(res.plan, state, { sdk, context: ctx, safety: baseSafety, budgets: budgets() });
  assert(sdk.calls.length === 1, 'charge invoked exactly once across two passes');
}

// ---------------------------------------------------------------------------
if (failures > 0) {
  console.error(`\n${failures} harness assertion(s) failed`);
  process.exit(1);
}
console.log('\nAll harness executor tests passed.');
