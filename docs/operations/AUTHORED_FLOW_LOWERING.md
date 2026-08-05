# Authoring executable Aelio flows

## Authority model

An SDK flow has two layers:

1. semantic `FlowSpec` says **what** must happen: activation, intent, postconditions,
   admissible capabilities, repair/escape policy, TTL and attempts;
2. `FlowLoweringV1` says **how data moves** through a closed Sol graph, but names calls only as
   `$cap:<binding>` symbols.

The TypeScript server does not resolve or execute those symbols. During catalog registration Rust:

1. validates the complete closed catalog;
2. admits exact versioned tool proxies with exact `pure|read|write|external` effects;
3. resolves every binding to exactly one capability/tool/version;
4. rejects zero/multiple matches, raw target ids, unused bindings, unknown semantic steps,
   excessive deadlines, unbounded cases, Park inside Loop, and Park counts above `max_attempts`;
5. rewrites the symbolic Sol graph to exact target pins;
6. applies TTL to kernel execution and the durable parked subject rail;
7. wraps runtime failures in the declared fail-closed `escalate` or `free_range` terminal;
8. runs 20–128 distinct fixture-only sandbox cases;
9. installs the immutable Flow pin into the same catalog snapshot; and
10. activates the TypeScript SDK host snapshot only after Rust accepts that catalog.

Format 1 refuses `escape.kind = fallback`. A safe fallback must atomically hand one durable subject
continuation to another exact artifact inside the reactor. Encoding that as a magic output for the
adaptive or TypeScript layer to interpret would create a second instruction authority. A future
lowering format may add the reactor-owned handoff; until then use `escalate`/`free_range` or keep the
flow semantic-only.

Without `aelio.lowering`, the semantic flow remains visible but non-executable. It does **not**
block catalog readiness (it is guidance, not an installed app). Flows that declare `lowering` must
pin during admission; until every such flow is pinned (and the SDK host is up when tools exist),
`/v1/health` reports `ready: false` and turns refuse to operate. A model may help author a candidate
off path; no model writes instructions into a live turn.

## SDK declaration

```ts
const cases = Array.from({ length: 20 }, (_, index) => ({
  input: { turn: { utterance: `+9198765432${String(index).padStart(2, '0')}` } },
  wakes: [{ turn: { utterance: '000000' } }, { turn: { utterance: '123456' } }],
  expect_park: false,
  expected: { text: 'Authenticated' },
  fixtures: [
    { binding: 'send', output: { ok: true }, usage_tokens: 0 },
    { binding: 'verify', output: { ok: true }, usage_tokens: 0 },
  ],
}))

aelio.flow('login', {
  state: 'unauthenticated',
  description: 'Login with a one-time code',
  steps: {
    collect_phone: { goal: 'Collect a valid phone', tool: 'sendOtp' },
    await_otp: { goal: 'Verify the submitted code', tool: 'verifyOtp' },
  },
  aelio: {
    version: '1',
    activation: {
      hard_preconditions: [{ op: 'eq', path: 'state', value: 'unauthenticated' }],
      trigger_surface: ['login', 'sign in', 'log in'],
      margin_threshold: 0.7,
    },
    learnable: false,
    preemption: 'hold',
    steps: [
      {
        id: 'collect_phone', intent: 'collect phone',
        postcondition: { op: 'present', path: 'slot.phone' },
        admissible: ['auth.otp.send'], on_violation: 'repair', suspendable: true,
      },
      {
        id: 'await_otp', intent: 'verify otp',
        postcondition: { op: 'present', path: 'evidence.otp_verified' },
        admissible: ['auth.otp.verify'], on_violation: 'repair', suspendable: true,
      },
    ],
    escape: { kind: 'escalate' },
    terminal_states: ['authenticated'],
    ttl_secs: 300,
    max_attempts: 3,
    lowering: {
      format: 1,
      program: {
        nid: 'login', op: 'Seq', steps: [
          {
            nid: 'send_once', op: 'Once', body: {
              nid: 'send', op: 'Call', id: '$cap:send',
              args: {
                args: { pull: 'turn' },
                context: { lit: { source: 'catalog.login' } },
              }, into: 'sent',
            },
          },
          { nid: 'ask', op: 'Const', v: { text: 'Enter the six-digit code' } },
          { nid: 'wait', op: 'Park', until: { kind: 'event' }, into: 'otp' },
          {
            nid: 'verify', op: 'Call', id: '$cap:verify',
            args: {
              args: { pull: 'otp.turn' },
              context: { lit: { source: 'catalog.login' } },
            }, into: 'verified',
          },
          { nid: 'done', op: 'Const', v: { text: 'Authenticated' } },
        ],
      },
      bindings: [
        { name: 'send', step_id: 'collect_phone', capability: 'auth.otp.send', deadline_ms: 30_000 },
        { name: 'verify', step_id: 'await_otp', capability: 'auth.otp.verify', deadline_ms: 30_000 },
      ],
      cases,
    },
  },
})
```

The abbreviated graph illustrates the contract. A real OTP graph should branch on verification,
take a typed repair edge, Park again without resending, and escape after the bounded final attempt.
The acceptance test `catalog_lowering_resolves_exact_capability_and_activates_runtime_owned_flow`
exercises that full shape, including database/runtime reopen between repair and completion.

## Version and deployment rules

- Changed program, binding, cases, effects, tool version or prompt means a new flow version.
- Byte-identical re-registration is idempotent and reuses existing admission evidence.
- Reusing a version with different immutable content fails `409`; it never silently rebinds.
- Rust admission completes before the new SDK connection becomes the callable host snapshot.
- A parked instance retains its artifact hash and target versions across restart.
- Expired subject rails are durably closed and no longer block a new activation.
- `Once` protects effect intent/result across Park, retry, crash and replay.
- A disconnected or failing pinned target takes the compiler-generated declared escape rail.

## What admission proves

The gate proves structural validity, bounded execution, fixture conformance, stable expected
properties and lifecycle requirements for supplied cases. It cannot prove that an author chose the
correct business meaning. Reviewed effects still require an identified deployer, canary evidence,
Guard monitoring and immediate conservative demotion.

## Deliberate non-goals (format 1)

- `escape.kind = fallback` — rejected until a reactor-owned continuation handoff format exists.
- Semantic-only `FlowSpec` without `aelio.lowering` — visible, non-executable, creates demand.
- Adaptive hot-path Pure/non-Park atomic invoke — suspendable work uses subject continuations.
- Legacy `World.user_flows` interpreter — parity/local worlds only; `AppState` constructors disable it.
- Live provider/load/soak — tracked by `PRODUCTION-LIVE-001` / Gate 5 of
  `docs/operations/PRODUCTION_READINESS_CHECKLIST.md`, not by local deterministic green alone.
