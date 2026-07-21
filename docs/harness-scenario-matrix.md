# Harness Scenario Matrix

This document records the current end-to-end scenario audit for the semantic
decision harness. The executable version is `scripts/test-scenario-matrix.mjs`
and can be run with:

```bash
pnpm test:scenarios
```

The script starts a temporary Sunjet ll-server, uses the real `processTurn`
runtime path, persists messages and traces into Sunjet, and validates both:

- the assistant reply shape, and
- the decision journal (`pathway`, `stance`, `prompt`, `reply`, `generic`,
  `cache`, and proactive hints where applicable).

The test uses deterministic local embeddings so failures are stable and point to
harness wiring or scoring behavior instead of provider variance.

## Coverage

| # | Scenario | User Message | Decision Observed | Reply Preview |
|---:|---|---|---|---|
| 1 | Generic greeting | `hi` | `generic=greeting`, `llm=0` | `Hi! How can I help you today?` |
| 2 | Generic thanks | `thanks!` | `generic=thanks`, `llm=0` | `You're welcome! Is there anything else I can help with?` |
| 3 | Frustrated VAT correction | `I am really frustrated - my invoice has the wrong VAT number again. Fix it immediately.` | `tool=update_invoice`, `sentiment=negative`, VAT memory reached prompt | `I understand the invoice/VAT issue. I will use the billing correction path and make sure any account change asks for confirmation.` |
| 4 | Refund request | `Order 8842 arrived damaged and I need a refund right now.` | `tool=refund_order`, `policy=confirm-writes` | `I can help with the refund path. I will check the order/refund context and keep confirmation in place for any change.` |
| 5 | Active plan flow | `This plan feels too expensive. Can you compare cheaper options?` | `flow=plan-change-flow`, `tool=compare_plans` | `I can compare the plan options and focus on value without pushing an upgrade.` |
| 6 | Confused login recovery | `I'm confused and locked out of login. I don't understand the reset step.` | `flow=account-recovery-flow`, `tool=reset_login`, `certainty=negative` | `I will help recover account access and keep the steps clear.` |
| 7 | Shipping lookup | `Where is my package? Please check the shipping status.` | `tool=get_order_status` | `I can check the shipping status and summarize the delivery state.` |
| 8 | Weather off-domain but valid tool | `What is the weather forecast for Paris tomorrow?` | `tool=get_weather` | `I can help with the weather lookup.` |
| 9 | Cache hit | `Can you explain what the invoice charge means?` | repeat turn served from cache, `llm=0` | cached billing explanation |
| 10 | Disengagement | `That's all, goodbye.` | `strategy=disengage`, `proactive=suppress` | `Goodbye. I will not start a new topic.` |
| 11 | Mixed sentiment span | `The dashboard is usable. But my invoice charge is still wrong and I am really frustrated.` | `sentiment=negative`, span attribution points at the invoice/frustration clause | billing correction response |

## What The Matrix Proves

- The generic gate bypasses embeddings and LLM calls for narrow contentless
  greetings/thanks.
- Billing/VAT memory reaches the compiled system prompt and affects the reply.
- Frustration is detected as negative sentiment and journaled.
- Write-sensitive pathways retain the hard confirmation policy.
- Active lifecycle state constrains flow selection.
- The semantic pathway can pick the correct tool across unrelated tools.
- The response cache can serve a repeat no-tool answer without another LLM call.
- Disengagement suppresses proactive re-engagement.
- Mixed messages can attribute stance to the specific negative span instead of
  blurring the whole message.

## Remaining Gaps Not Covered By This Matrix

- Real provider embedding calibration.
- Explicit temporal resolver for phrases like `last week` or `when I last asked`.
- Full typed atom/span contract shared by pathway, memory, policy, flow, and axis
  recall.
- User-specific Harness Axis graph with occurrence chains.
- Graph-constrained hybrid recall and unified evidence scoring.
- Admin aspect governance.
- Prompt trace retention/redaction policy in production.

