# Implementation Verification — Interrogation Checklist

**Purpose:** verify a Cursor-built implementation against v4 + the seven build findings. Paste sections at Cursor, or run the commands yourself where given.

**How to use this well:** for every claim of "yes, implemented," demand the **file path and the test name** that proves it. An implementation without a failing-case test is a claim, not a fact. The questions are phrased to make "show me" the default. Anything answered with prose and no pointer goes on the open list.

Three severity tiers:
- **[KILL]** — wrong here invalidates everything downstream; every trace and signature produced before the fix is garbage. Stop and fix before writing more code.
- **[SEC]** — wrong here is a data leak or privilege escalation in a product sold on neither happening.
- **[CORR]** — wrong here produces confident wrong answers, the failure class the whole system exists to prevent.

---

## SECTION 0 — Orientation (ask these first)

These calibrate how much to trust everything else it says.

1. List every module you have implemented, with its file path and the v4 section it implements. Which sections have NO implementation yet?
2. For each module: how many tests, and how many assert a **failure** case (an error, a rejection, a divergence) rather than a success? A suite that only tests happy paths has not tested this system.
3. What is NOT implemented that the code silently pretends is? Stubs returning `Ok`, TODOs on security paths, `unimplemented!` behind flags. List every one.
4. Did you deviate from the spec anywhere? Where, and why? *(Deviations may be fine — undocumented deviations are not.)*
5. Show me `Cargo.toml` and the full dependency tree: `cargo tree`.

---

## SECTION 1 — Determinism [KILL]

The M1 exit criterion. If any of these fail, nothing else in this document matters yet.

### Hashing and serialisation
6. Is `output_hash` computed by a dedicated canonical serialiser, or by `serde_json`/serde anywhere? Show the module. *(§2.9: serde is disqualifying — a serde version bump must never change a hash.)*
7. Run: `grep -rn "serde_json::to_vec\|serde_json::to_string" src/` — anything in a hashing path?
8. How are floats serialised for hashing? *(Shortest round-trip / ryu, pinned.)* Show the test proving `0.1 + 0.2` hashes differently from `0.3`.
9. What happens when `-0.0` is hashed? Show the test asserting it equals `0.0`'s hash.
10. What happens when a `NaN` reaches the serialiser? *(Trick question — it must be unreachable; ops producing NaN return `Err`. Show `guard_float` or equivalent, and the test where `Infinity + -Infinity` errors.)*
11. Show the test where `["ab","c"]` and `["a","bc"]` hash differently. *(Length-prefixing.)*
12. Do `Int(1)` and `Decimal(1.000000)` hash identically or differently? *(Must differ — type is part of identity.)*

### Collection order
13. Run: `grep -rn "HashMap\|HashSet" src/ --include="*.rs" | grep -v "BTree"` — every hit in the value, hashing, or effect path is a bug. *(std's hasher is randomly seeded PER PROCESS; this passes all local tests and diverges in nightly replay with no visible cause.)*
14. Is there a clippy `disallowed_types` config so the next contributor can't reintroduce it? Show `clippy.toml`.
15. Run: `cargo tree -e features | grep preserve_order` — if serde_json's `preserve_order` is enabled by ANY transitive dependency, map key order becomes the source database's whim.

### Floats
16. How is a float column summed? *(Pairwise, recursion order fixed by INPUT INDEX — never accumulation in arrival order.)* Show the function.
17. Show the test asserting that reordering `[1e16, 1.0, -1e16]` **changes** the sum. *(This documents the hazard that justifies the ordering rules. If it doesn't exist, the implementer doesn't know why the rules exist.)*
18. Any fast-math flags anywhere? Is there a CI check that fails if one appears?
19. Is `f64::mul_add` used in aggregation? *(FMA contraction is the likely cause if hashes diverge on ARM.)*
20. Have reference-vector hashes been verified on **both x86_64 and aarch64**, debug AND release? Show the CI run. If not: this is the M1 exit gate and it is open.

### Replay
21. Show `verify_replay`. Does it compare **every step hash**, not just final output? *(Final-only localises nothing on failure.)*
22. What happens when a replayed execution reaches an effect the original journal doesn't contain? *(Must be `JournalUnderrun`. If replay can DISPATCH, replays have side effects and the audit story dies — finding N15.)* Show the test.
23. Is there a nightly job (or documented plan) replaying sampled production traces with hard alerts on divergence?
24. Are journal entries for `map_tool` written in **input-index order** regardless of completion order? Show the test with induced out-of-order completion.

---

## SECTION 2 — Numeric tower [CORR]

25. Show `add(Int, Float)`. *(Must be a TYPE ERROR, not a widening.)*
26. What does `div(Int, Int)` return? *(Decimal — never Float, never truncated Int. `1/3` must not become a binary float. Integer division is a SEPARATE op, `div_floor`.)*
27. What is `mean` over `Int` inputs? *(`Decimal{6}` — averaging counts and getting a binary float back is how rounding error enters a system that had none.)* Show the §2.2 return-type table encoded in code.
28. What rounding mode does `Decimal` division use, and where is target scale decided? *(Banker's/half-even, declared per op. Build finding: the spec left target scale open — what did you pick, and is it ONE rule or per-call-site improvisation?)*
29. What happens when `Decimal{s+6}` exceeds `MAX_SCALE`? *(Build finding: needs an explicit clamp policy, not a silent one.)*
30. Show the test: division by zero returns `Err` — not `Inf`, not a panic — for all three numeric types.
31. Is `Money` representable as `Float` anywhere? *(Must not be constructible.)*
32. Overflow on `Int` sum: checked or wrapping? Show `checked_add` and its test past 2^53.

---

## SECTION 3 — Null, Empty, Count [CORR]

33. Does a bare `count` op exist? *(It must NOT. Only `count_rows` and `count_values`. A bare `count` is the name readers assume and get wrong, and getting it wrong changes every mean in the system.)*
34. `count_rows([1, null, 3])` and `count_values([1, null, 3])` — show both answers and the test. *(3 and 2.)*
35. Is null policy in the **op name** (`mean_skip_null` / `mean_strict` / `mean_zero_null`) or a parameter? *(Name. Policy-as-parameter makes null handling a hole, and two workflows differing only in null handling would share a skeleton — a silent-wrong-answer merge.)*
36. Run the golden case: `mean` over `[1, null, 3]` under all three policies. Expected `2.0` / refusal / `1.333333`. Show the test.
37. What does `mean_strict` over a null return — `Err`, `Empty`, or a distinct refusal? *(Build finding: fourth variant, `NullEncountered`. "This column has nulls and you asked for strict" beats a generic error.)*
38. What is `mean` of an empty or all-null collection? *(`Empty` — never `0`, never NaN-div.)*
39. What is `count_rows([])`? *(`Ok(0)` — the ONE aggregate where zero is correct: zero is the right cardinality of nothing.)*
40. Show the test: `Empty` propagates through arithmetic — `div(Empty, 5)` is `Empty`, not zero, not an error.
41. Does `Empty` carry the narrowest filter that produced it, so the renderer can say "no people over 28" rather than "no results"?
42. Do distinct `Empty` reasons hash differently?

---

## SECTION 4 — Budget [CORR]

43. Is the budget ONE pool per turn shared by reference down the call stack, or per-frame? *(Per-frame is the v2 bug: ten nested harnesses × ten fresh allowances. Show the test where nested frames drain one shared pool.)*
44. Is depletion CAS or `fetch_sub`? *(Finding N14: saturating subtract lets concurrent `map_tool` elements overshoot before any observes exhaustion. Show the 8-thread test asserting the drained total EXACTLY equals the limit.)*
45. What does `model_calls` default to? *(ZERO. Raising it requires promotion approval.)*
46. On exhaustion, does the error name the **dimension AND the frame**?
47. Is partial output possible under exhaustion in ANY path? *(Never — a partial aggregate is indistinguishable from a correct one. Show the abort path.)*
48. Boundary cases (open matrix cell PRT×5): budget of exactly 0 — unlimited or immediately exhausted? Taking exactly the remaining amount? Show tests.
49. Do audit executions get a **separate** pool, so an audit can never cause a user-facing timeout?
50. Does `result_rows` exist as a dimension? *(Trips before the heap limit and produces an actionable message.)*

---

## SECTION 5 — Taint [SEC]

51. How does external data enter the value graph? *(ONLY through a `from_tool`-style constructor that taints. If a tool result can be built as a plain value, taint is decorative.)*
52. Propagation: output taint = join of input taints for EVERY op? Enforced at one chokepoint, or per-op discipline? *(Per-op discipline will be forgotten.)*
53. **The map-key question (finding N13):** `group_by(tainted_records, "city")` produces a map keyed by tainted strings. Where does the key's taint go? *(The key position in `BTreeMap<String, Value>` has no taint field — the container must ABSORB key taint, and extracting a key must return it tainted. Show both tests. "Tainted keys are forbidden" is the wrong answer — group_by is legitimate. Silence means the bit is being laundered.)*
54. Does a clean element extracted from a tainted container come back tainted? *(Container taint is a lower bound on contents — conservative in the correct direction.)*
55. **The implicit-flow question (finding N12):** walk me through —
    if record.salary > 100000 (tainted predicate): emit("high_earner", {}) — clean payload; else: emit("normal", {}).
    Both payloads pass value-taint checks. Which event fires is one bit of the predicate; a loop emits one bit per iteration. What catches this? *(PC-taint: statements in a region whose predicate is tainted inherit the taint; `emit`/`fail` are prohibited there. If the implementer has never heard of implicit flows, re-review their whole taint layer.)*
56. Does a nested CLEAN predicate clear inherited PC taint? *(Must not. Show the test.)*
57. Enumerate the prohibited positions and show ONE test iterating ALL of them against a tainted value: ToolName, HarnessRef, ModelPrompt, AuthoringPrompt, PermissionDecision, IdempotencyKey, JoinPath, EmitPayload, FailReason. Any one missing is a channel.
58. Can `fail(reason)` contain a tainted value? *(An error message is a channel: "no results for city=Bangalore for user rajesh@…" in your logs is the same leak by another route.)*
59. Is `declassify_count` the ONLY untainting operation, returning cardinality and nothing else, visible in the AST?
60. Adversarial: taint a value, wrap it two lists deep, take its length, compare it to a constant, put the comparison result in an idempotency key. Which step rejects? *(Something must.)*

---

## SECTION 6 — Versioning and authority [SEC] / [KILL]

61. Does `SystemVersion` exist as ONE struct whose `hash()` **exhaustively destructures** it, so adding a field without hashing it fails to compile? Prove it: add a dummy field, `cargo check`, show the error, remove it.
62. Show the test asserting every field perturbs the hash.
63. Is the renderer's version in `SystemVersion`? *(Finding N4: concrete source is RENDERED from the AST, so a whitespace change alters every harness hash. Unversioned renderer = a refactor silently orphans every cached plan.)*
64. Are op IMPLEMENTATIONS hashed individually, with the catalog version as their Merkle root? *(Finding N5: a fix to `median`'s tie-breaking changes behaviour with no signature change — nothing invalidates, no audit boundary. `impl_hash` makes a bug fix correctly painful.)*
65. Does `Verified<T>` exist with **no public constructor**, so passing a raw graph-query result to `authorize()` is a **compile error**? Trybuild compile-fail tests written? If not, they're the actual closure — write them first.
66. Grep for `into_inner_unchecked` (or your escape hatch) — justify every call site.
67. Is the effect set read at execution from the **content-addressed cache**, never a live graph query — and does the kernel independently walk the AST it's about to execute and assert direct tool refs are a subset of the cached set? *(I5. A stale index must be a loud failure, not a silent escalation.)* Show the corrupted-cache test.
68. Is the dialect config hash **asserted at startup**? What if `enable_recursion` or `enable_sets` got flipped? *(Must refuse to start.)*

---

## SECTION 7 — Effect boundary [SEC] / [CORR]

*(If the effect driver isn't built, mark deferred — but ask 69–71 anyway; they're contract-shape decisions that get baked in early.)*

69. Does every tool contract require `effect_class`, `completeness`, `returns_entity` at registration — with registration REFUSING without them?
70. Is `Completeness::Unknown` (and `Sampled`) a promotion blocker for any aggregate-computing workflow over that tool? *(This one check prevents the whole silently-wrong-aggregate class from paginated sources.)*
71. Is `row_scope` a required **positional** on `row_scoped=true` tools, so forgetting authorisation is a type error?
72. Shadow mode: `Write` and `IdempotentWrite` **hard-stubbed**, non-overridable? *(The v2 bug: shadow runs ALONGSIDE live — an unstubbed write fires twice on every shadow execution. Double-charge, double-ticket.)* Show the test.
73. Is the stub LOUD — synthetic success plus `stubbed: true` journal entry — so shadow comparison can exclude write-dependent branches?
74. Idempotency keys: `(turn_key, effect_seq, element_index)`? *(Finding N6: without `element_index`, all N elements of one `map_tool` share a key and a downstream idempotent tool DEDUPLICATES them into one write.)*
75. Is `turn_key` client-supplied and retry-stable — and does a repeated `turn_key` with a DIFFERENT body get rejected loudly rather than served the cached result?
76. `map_tool`: all-or-nothing? *(97 of 100 results into an aggregate is a confident wrong number.)* Is lenient mode a separate AST-visible op, never a flag?
77. Are writes EVER auto-retried? *(Never. Only Read and IdempotentWrite.)*
78. Does the kernel canonically ORDER every tool result by entity key before it enters the value graph, unconditionally? For paginated tools, does the KERNEL drive pagination via a declared `cursor_key`, with page-continuity checks failing `UnstableSource` on mid-scan mutation? *(Finding B1: sorting a nondeterministically-sampled page is a deterministic ordering of the wrong rows.)*
79. Can a harness ever see an individual page? *(Never — complete result or budget error.)*
80. `emit`: closed event enum? Scalar-only payloads? Taint-checked? Statically rejected if the payload contains a collection?

---

## SECTION 8 — Starlark embedding [KILL if wired, else deferred]

81. Which starlark-rust version, and is the dialect locked — `enable_recursion=false`, `enable_sets=false`, `enable_top_level_stmt=false`, globals frozen? Show the config AND the startup assertion.
82. Is the evaluator's value type the tainted wrapper, or raw values with taint bolted on elsewhere? *(Bolted-on taint has seams.)*
83. How does the sync evaluator reach async effects? *(spawn_blocking + mpsc/oneshot bridge. NOT continuation suspension — unsupported, multi-week trap. NOT a nested runtime.)*
84. Is the budget threaded through so STEPS deplete the shared pool — and does the starlark heap limit actually enforce or merely approximate? Tested with a program allocating past the limit?
85. Cross-harness cycles: Starlark forbids recursion within a module but can't see across `call_harness`. Runtime call-stack check on content hashes PLUS the static DAG check? Show A→B→A rejected at runtime.
86. Depth cap enforced kernel-side at 16?

---

## SECTION 9 — Things the spec knows about that you might not have asked Cursor

Even for unbuilt components, ask whether anything already built FORECLOSES them.

87. **Time:** all internal time UTC epoch ms, tzdb PINNED and SHIPPED (never host-read), version in `SystemVersion`? Intervals half-open `[start, end)` everywhere?
88. **Relative time:** does anything cache a resolved date where the relative expression + tenant-tz validity bucket should be? *(A `LastNDays(30)` plan cached with resolved dates serves yesterday's window tomorrow. The bucket flips at TENANT midnight, not UTC — the off-by-5.5-hours bug for an Asia/Kolkata tenant.)*
89. **Collation:** string ordering byte-wise UTF-8, no locale, no ICU, no implicit NFC? Human collation confined to the renderer, after hashing?
90. **Errors in the value path:** structured enums, NO free-form text? *(Finding N3: a Debug rendering with a pointer, duration, or HashMap enters `output_hash`.)* Grep for `format!` feeding anything hashed.
91. **`Reply` path:** structurally NO effect-driver access? Numeric-claim post-check forcing re-triage? *(Everything is downstream of triage being right, and triage was the one ungated component.)*
92. **Session serialisation:** per-session mutex, ledger append INSIDE the lock, bounded queue rejecting beyond depth 2 rather than backlogging stale computations?
93. **Ledger:** hash-chained with `prev_hash`? Single-instance guard REFUSING a second instance against the same store rather than silently forking?
94. **Renderer purity:** can the answer renderer do ANY arithmetic, or is it templating only? *(If it can recompute a number, determinism ends at the last layer, invisibly.)*
95. **Verification renderer:** same AST traversal as execution rendering, with a property test that every effect / discriminating binding / join path / limit appears? Two renderings drift; one cannot.
96. **Skeleton normalisation** (if implemented): commutative reordering restricted to Int/Bool/String operands, with Float and Decimal NEVER reordered? *(Float reordering makes two skeletons hash identically and compute differently; equivalence merging then aliases them — I1 broken by the abstraction layer itself.)*
97. **Residue check** (if matching exists at all): run the canonical case — "average height of people over 28 **in Bangalore**" against a single-filter-slot skeleton. It must REJECT on the unconsumed city constraint, not bind cleanly and drop it. **This is the single most important behaviour in the system.**
98. **Fan-out** (if join paths exist): 1:N edges non-traversable for binding, requiring an explicit quantifier compiling to a semi-join? Run: aggregate over people→addresses where someone has 3 addresses — the person counts once.
99. **Ambiguity:** billing_address→city vs shipping_address→city — Clarify, or is a tie-break (shortest, first-found) hiding anywhere? *(Any tie-break here is a silent-wrong-answer generator with a plausible rule in front of it.)*

---

## SECTION 10 — Meta-questions about the code itself

100. Run `cargo test` and `cargo test --release` — do ALL hash-producing tests match across both? *(Optimisation-level divergence is finding-grade.)*
101. What panics can reach production? `grep -rn "unwrap()\|expect(\|panic!" src/ | wc -l` — justify each in a security or hashing path.
102. Any `unsafe`? Each needs a written invariant.
103. **What did YOU find ambiguous in the spec, and what did you decide unilaterally?** *(Highest-yield question in this document. All seven build findings so far came from exactly these gaps — division scale, MAX_SCALE clamp, NullEncountered, CAS, JournalUnderrun, map-key taint, implicit flow. Cursor's unilateral decisions are the next batch, in either direction: good ones to ratify into the spec, bad ones to fix.)*
104. If I deleted your test suite, what incorrect implementation would still compile and run without visible error? *(The honest answer tells you where the NEXT test needs to be.)*

---

## Scoring

- Any **[KILL]** failure → stop feature work; fix; re-verify the whole KILL section. Traces and signatures from before the fix are invalid.
- Any **[SEC]** failure → fix before the code touches any real tool or credential.
- **[CORR]** failures → fix before matching/reuse is enabled; execution-only paths may proceed.
- Q103's answers → triage each unilateral decision into "ratify into spec" or "fix," and amend v4 either way. Undocumented decisions are how two implementations drift.

If more than ~15 come back wrong or unanswerable, the implementation drifted early — cheaper to re-align now against `harness-core` (which passes all 54 of its own versions of these) than to keep building on the drift.
