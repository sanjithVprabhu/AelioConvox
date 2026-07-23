# Aelio DSL — Complete Instruction Dictionary

Version 0.1 — full-level enumeration (compute-facing).

---

## 0. Conventions

### 0.1 Type universe

| Type | Meaning |
|---|---|
| `Int` | i64 |
| `Decimal` | fixed-point, money-safe |
| `Float` | f64 |
| `Str` | UTF-8 string |
| `Bool` | boolean |
| `Bytes` | opaque byte buffer |
| `Instant` | absolute point in time (UTC) |
| `Duration` | signed span |
| `Interval` | `{start: Instant, end: Instant}` |
| `Tz` | IANA timezone id |
| `Value` | dynamic JSON-shaped tree |
| `TypeTag` | runtime type discriminant |
| `List<T>` | ordered sequence |
| `Map<K,V>` | keyed map |
| `Option<T>` | present / absent |
| `Result<T,E>` | success / typed failure |
| `Vector` | dense f32 embedding |
| `Id` | opaque stable identifier |
| `Path` | structure access path (see 2.6) |
| `Pattern` | pre-compiled linear-time regex |

### 0.2 Universal rules

1. **Totality.** Every op returns `Result<T, ReasonCode>`. Nothing throws. Nothing returns a null that means two different things.
2. **No silent coercion.** `Int → Decimal → Float` conversions are explicit ops.
3. **Explicit timezone.** Every calendar-facing time op takes a `Tz`. There is no implicit local time.
4. **Bounded everything.** Loops, repeats, retries, fan-out, and recursion all carry a declared ceiling.
5. **Named args on non-commutative ops.** `Subtract{from, by}`, never `Subtract(a, b)`.
6. **Variadic on associative ops.** `Add[n...]`, not `Add` + `AddPlus`.
7. **One contract at every rung.** See §9.1. A caller cannot tell whether it invokes a primitive or a 100-step flow.

### 0.3 Notation

```
Name{named: Type, ...} -> Result<Out, E>        record args
Name[Type...]          -> Result<Out, E>        variadic
Name<Op>{...}          -> ...                   higher-order (combinator)
```

Substrate marks: `P` pure · `S` semantic · `L` llm · `E` effect
`⇄` = has both a cheap (S) and expensive (L) implementation; escalation governed by `Judge.Confidence`.

---

## 1. S0-A — Combinators

Closed set. Never extended by tenants. Kept small because the combinator set is the branching factor of tier-2 compositional search.

### 1.1 Control shape

| Op | Signature | Notes |
|---|---|---|
| `Seq` | `Seq<Op...>{input} -> Result<Out>` | left-to-right; output of *n* feeds *n+1*; short-circuits on first `Err` |
| `Par` | `Par<Op...>{input, merge: MergeRule, max_concurrency: Int} -> Result<Out>` | requires declared merge; slot-write conflicts resolved by `MergeRule` |
| `Branch` | `Branch{pred: Op<Bool>, then: Op, else: Op} -> Result<Out>` | both arms must have unifiable output types |
| `Switch` | `Switch{on: Op<Label>, cases: Map<Label,Op>, default: Op}` | closed label set only |
| `Loop` | `Loop<Op>{init, while: Op<Bool>, max_iter: Int} -> Result<Out>` | `max_iter` **mandatory**; exceeding it → `Err(LoopBudgetExceeded)` |
| `ForEach` | `ForEach<Op>{list: List<T>, max_items: Int, mode: Serial\|Parallel(n)}` | |
| `Try` | `Try{body: Op, catch: Map<ReasonCode, Op>, finally: Op?}` | catch keyed on closed reason codes, not exception types |
| `Retry` | `Retry{body: Op, policy: RetryPolicy}` | `RetryPolicy{max_attempts, backoff: Fixed\|Exp{base,cap}, jitter: Bool, retry_on: [ReasonCode]}` — only idempotent bodies may be retried; enforced structurally |
| `Guard` | `Guard{invariant: Op<Bool>, body: Op, on_violation: ReasonCode}` | pre/post assertion wrapper |
| `Timeout` | `Timeout{body: Op, ms: Int}` | → `Err(Timeout)` |
| `Budget` | `Budget{body: Op, tokens: Int?, ms: Int?, calls: Int?}` | nested budgets intersect, never widen |
| `Fallback` | `Fallback<Op...>{input}` | first non-`Err` wins; ordered by cost class |
| `Race` | `Race<Op...>{input, ms}` | first success wins; losers cancelled; **only pure/read-only ops admissible** |
| `Memo` | `Memo{body: Op, key: Op<Str>, ttl: Duration}` | pure bodies only |
| `Debounce` | `Debounce{body: Op, key: Str, window: Duration}` | outbound/proactive guard |
| `Once` | `Once{body: Op, idem_key: Str}` | ledger-backed; the effectful-dedup primitive |

### 1.2 Higher-order collection ops

These are structurally combinators (they take an `Op` argument), even though they read like list utilities.

| Op | Signature |
|---|---|
| `Map` | `Map<Op>{list: List<A>} -> Result<List<B>>` |
| `Filter` | `Filter<Op<Bool>>{list: List<A>} -> Result<List<A>>` |
| `Reduce` | `Reduce<Op>{list: List<A>, init: B} -> Result<B>` |
| `FlatMap` | `FlatMap<Op>{list: List<A>} -> Result<List<B>>` |
| `Any` | `Any<Op<Bool>>{list} -> Result<Bool>` |
| `All` | `All<Op<Bool>>{list} -> Result<Bool>` |
| `Find` | `Find<Op<Bool>>{list} -> Result<Option<A>>` |
| `Partition` | `Partition<Op<Bool>>{list} -> Result<{yes,no}>` |
| `SortBy` | `SortBy<Op<Key>>{list, dir} -> Result<List<A>>` |
| `GroupByOp` | `GroupByOp<Op<Key>>{list} -> Result<Map<Key,List<A>>>` |

### 1.3 Data-flow plumbing

| Op | Signature | Notes |
|---|---|---|
| `Let` | `Let{bindings: Map<Name,Op>, body: Op}` | local names, lexical scope |
| `Pipe` | `Pipe<Op...>` | sugar for `Seq` with implicit threading |
| `Tee` | `Tee{body: Op, side: Op}` | side result discarded; used for ledger writes |
| `Const` | `Const{value: Value}` | lift a literal into op position |
| `Identity` | `Identity{input} -> input` | |

---

## 2. S0-B — Pure operations

Total, side-effect-free, deterministic, cost class `free`. This layer is deliberately large and boring: it is what makes deterministic glue possible instead of reaching for a model to reshape a payload.

### 2.1 Numeric

| Op | Signature |
|---|---|
| `Add` | `Add[Num...] -> Result<Num>` |
| `Subtract` | `Subtract{from: Num, by: Num} -> Result<Num>` |
| `Multiply` | `Multiply[Num...] -> Result<Num>` |
| `Divide` | `Divide{from: Num, by: Num} -> Result<Num, DivByZero>` |
| `Modulo` | `Modulo{of: Num, by: Num} -> Result<Num, DivByZero>` |
| `Power` | `Power{base: Num, exp: Num} -> Result<Num, Overflow>` |
| `Sqrt` | `Sqrt{value} -> Result<Num, DomainError>` |
| `Log` | `Log{value, base} -> Result<Float, DomainError>` |
| `Abs` | `Abs{value} -> Result<Num>` |
| `Negate` | `Negate{value} -> Result<Num>` |
| `Sign` | `Sign{value} -> Result<Int>` (−1/0/1) |
| `Round` | `Round{value, places: Int, mode: HalfUp\|HalfEven\|Up\|Down} -> Result<Num>` |
| `Floor` / `Ceil` | `{value} -> Result<Int>` |
| `Truncate` | `Truncate{value, places} -> Result<Num>` |
| `Min` / `Max` | `[Num...] -> Result<Num>` |
| `Sum` / `Avg` | `[Num...] -> Result<Num>` (`Avg` on empty → `Err(EmptyInput)`) |
| `Median` / `Percentile` | `{list, p} -> Result<Num>` |
| `Clamp` | `Clamp{value, lo, hi} -> Result<Num>` |
| `Compare` | `Compare{a, b} -> Result<Lt\|Eq\|Gt>` |
| `InRange` | `InRange{value, lo, hi, inclusive: Bool} -> Result<Bool>` |
| `Increment` / `Decrement` | `{value, by: Num = 1} -> Result<Num>` |
| `Ratio` | `Ratio{part, whole} -> Result<Decimal, DivByZero>` |
| `PercentOf` | `PercentOf{value, pct} -> Result<Decimal>` |

Overflow is an error, never a wrap. `Decimal` is the default for anything money-shaped.

### 2.2 Type conversion

`ToInt{value} -> Result<Int, ParseError|Lossy>` ·
`ToDecimal{value, scale?}` ·
`ToFloat{value}` ·
`ToBool{value, truthy_set?}` ·
`ToStr{value, format?}` ·
`ParseNumber{string, locale?} -> Result<Num, ParseError>` ·
`TypeOf{value} -> TypeTag` ·
`IsType{value, tag} -> Bool` ·
`Cast{value, tag} -> Result<Value, CastError>`

### 2.3 String

| Op | Signature |
|---|---|
| `Length` | `{value: Str} -> Int` (grapheme count, not bytes) |
| `ByteLength` | `{value} -> Int` |
| `Strip` / `StripLeft` / `StripRight` | `{value, chars: Str?}` |
| `Lower` / `Upper` / `TitleCase` / `SentenceCase` | `{value, locale?}` |
| `Slice` | `{value, start: Int, end: Int?}` — out-of-range **clamps**, never errors |
| `CharAt` | `{value, index} -> Option<Str>` |
| `IndexOf` / `LastIndexOf` | `{haystack, needle} -> Option<Int>` |
| `Contains` / `StartsWith` / `EndsWith` | `{value, needle} -> Bool` |
| `Replace` | `{value, find, with, count: Int?}` |
| `Concat` | `[Str...] -> Str` |
| `Join` | `{parts: List<Str>, sep: Str}` |
| `Pad` | `{value, len, char, side: Left\|Right\|Both}` |
| `Repeat` | `{value, n, max_len: Int}` — `max_len` mandatory |
| `Reverse` | `{value}` (grapheme-safe) |
| `Truncate` | `{value, len, ellipsis: Str?}` |
| `Normalize` | `{value, form: NFC\|NFD\|NFKC\|NFKD}` |
| `RemoveDiacritics` | `{value}` |
| `Slugify` | `{value}` |
| `IsEmpty` / `IsBlank` | `{value} -> Bool` |
| `EqualsIgnoreCase` | `{a, b} -> Bool` |
| `Levenshtein` | `{a, b} -> Int` |
| `SimilarityRatio` | `{a, b} -> Float` (deterministic string distance, not embedding) |

### 2.4 Splitting

Five distinct ops. Word-splitting is not one thing.

| Op | Signature | Case |
|---|---|---|
| `SplitByDelimiter` | `{value, sep, limit: Int?} -> List<Str>` | `"a,b,c"` |
| `SplitByWhitespace` | `{value} -> List<Str>` | sentence → words; collapses runs, handles tab/newline |
| `SplitByRegex` | `{value, pattern} -> List<Str>` | |
| `SplitByLength` | `{value, n} -> List<Str>` | fixed-width chunks |
| `SplitLines` | `{value} -> List<Str>` | normalizes `\r\n`/`\r`/`\n` |
| `SplitSentences` | `{value, locale} -> List<Str>` | abbreviation-aware; still deterministic |

Note: `SplitByWhitespace` does not handle `"don't"`, `"co-founder"`, `"₹1,500"` semantically. Anything needing linguistic tokenization is an **ability**, not a pure op.

### 2.5 Regex

`RegexMatch{value, pattern} -> Bool` ·
`RegexFind{value, pattern} -> Option<Match>` ·
`RegexFindAll{value, pattern, max: Int} -> List<Match>` ·
`RegexCapture{value, pattern} -> Result<Map<Str,Str>, NoMatch>` ·
`RegexReplace{value, pattern, with, count?}` ·
`RegexSplit{value, pattern}` ·
`RegexEscape{value} -> Str`

Constraints: patterns compiled and validated at registration time; engine must be linear-time (no backtracking) — a tenant-supplied pattern is otherwise a DoS vector.

### 2.6 Structure (JSON / document)

**Path syntax** (deliberately minimal — not full JSONPath):
```
a.b.c        dotted keys
a[0]         index
a[*]         wildcard over list
a["x.y"]     quoted key containing a dot
```
No filters, no expressions, no recursive descent.

| Op | Signature |
|---|---|
| `GetPath` | `{doc: Value, path: Path} -> Option<Value>` |
| `GetPathAll` | `{doc, path} -> List<Value>` (wildcard) |
| `SetPath` | `{doc, path, value} -> Result<Value>` |
| `DeletePath` | `{doc, path} -> Result<Value>` |
| `HasPath` | `{doc, path} -> Bool` |
| `Keys` / `Values` / `Entries` | `{obj} -> List<...>` |
| `Pick` / `Omit` | `{obj, keys: List<Str>}` |
| `MergeShallow` | `{a, b}` |
| `MergeDeep` | `{a, b, conflict: TakeLeft\|TakeRight\|Error}` |
| `Flatten` | `{obj, sep: Str} -> Map<Str,Value>` |
| `Unflatten` | `{map, sep} -> Result<Value>` |
| `ParseJson` | `{string} -> Result<Value, ParseError>` |
| `ToJson` | `{value, pretty: Bool} -> Str` |
| `ParseCsv` | `{string, sep, header: Bool} -> Result<List<Map>>` |
| `ToCsv` | `{rows, cols} -> Result<Str>` |
| `ParseKv` | `{string, pair_sep, kv_sep} -> Map<Str,Str>` |
| `Shape` | `{value} -> ShapeSig` — structural fingerprint (see §4.6) |
| `Diff` | `{a, b} -> List<Change>` |
| `Patch` | `{doc, changes} -> Result<Value>` |

`GetPath` is the workhorse: `$.otp` extraction is exactly this op.

### 2.7 Collections

`First` / `Last` / `Nth{list,i}` → `Option<T>` (never error) ·
`Take{list,n}` · `Drop{list,n}` · `Slice{list,start,end}` ·
`Count{list}` · `IsEmpty{list}` ·
`Append{list,item}` · `Prepend` · `InsertAt{list,i,item}` · `RemoveAt` ·
`Concat[List...]` · `Flatten{list, depth}` ·
`Reverse{list}` · `Sort{list, key: Path?, dir}` ·
`Dedupe{list, key: Path?}` · `GroupBy{list, key: Path}` ·
`Zip[List...]` · `Unzip` · `Chunk{list,n}` · `Window{list,n,step}` ·
`Union` / `Intersect` / `Difference` `{a,b,key?}` ·
`IndexOfItem{list,item} -> Option<Int>` · `ContainsItem -> Bool` ·
`Range{from,to,step} -> List<Int>` ·
`Sample{list,n,seed}` — deterministic given seed

### 2.8 Time

All calendar-facing ops take an explicit `Tz`.

| Op | Signature |
|---|---|
| `DateAdd` | `{instant, amount: Int, unit: TimeUnit, tz} -> Result<Instant>` |
| `DateSub` | `{instant, amount, unit, tz}` |
| `DateDiff` | `{a, b, unit} -> Result<Int>` |
| `DateTrunc` | `{instant, unit, tz} -> Instant` |
| `Format` | `{instant, pattern, tz, locale} -> Str` |
| `ParseDate` | `{string, pattern, tz} -> Result<Instant, ParseError>` |
| `StartOfDay` / `EndOfDay` | `{instant, tz}` |
| `StartOfWeek` / `StartOfMonth` | `{instant, tz, week_start}` |
| `DayOfWeek` / `DayOfMonth` / `MonthOf` / `YearOf` | `{instant, tz} -> Int` |
| `IsWeekend` | `{instant, tz, weekend_days} -> Bool` |
| `MakeInterval` | `{start, end} -> Result<Interval, InvertedInterval>` |
| `IntervalContains` | `{interval, instant} -> Bool` |
| `IntervalOverlaps` | `{a, b} -> Bool` |
| `IntervalDuration` | `{interval} -> Duration` |
| `IntervalIntersect` / `IntervalUnion` | `{a, b} -> Option<Interval>` |
| `DurationOf` | `{amount, unit} -> Duration` |
| `DurationAdd` / `DurationScale` | |
| `HumanizeDuration` | `{duration, locale} -> Str` ("3 days ago") |
| `ConvertTz` | `{instant, from, to}` |

`TimeUnit = ms | s | min | hour | day | week | month | quarter | year`
`Now` is **not** here — it is an effect (§3).

### 2.9 Logic & predicates

`And[Bool...]` · `Or[Bool...]` · `Not{value}` · `Xor{a,b}` ·
`Equals{a,b}` · `NotEquals` · `DeepEquals{a,b}` ·
`IsNull{value}` · `IsPresent{value}` · `Coalesce[Value...]` ·
`IfElseValue{cond, then: Value, else: Value}` — value-level only; the op-level branch is `Branch` in S0-A ·
`Matches{value, constraint: Constraint} -> Result<Bool, Violation>` ·
`AllOf` / `AnyOf` / `NoneOf` `{value, constraints}`

### 2.10 Validation

| Op | Signature |
|---|---|
| `ValidateType` | `{value, tag} -> Result<Value, TypeViolation>` |
| `ValidateRange` | `{value, lo, hi} -> Result<Num, RangeViolation>` |
| `ValidateLength` | `{value, min, max} -> Result<Str, LengthViolation>` |
| `ValidateEnum` | `{value, allowed} -> Result<Value, EnumViolation>` |
| `ValidatePattern` | `{value, pattern} -> Result<Str, PatternViolation>` |
| `ValidateFormat` | `{value, format: Email\|E164\|Url\|Uuid\|Iso8601\|Ipv4\|CreditCard} -> Result<Str, FormatViolation>` |
| `ValidateSchema` | `{value, schema: Schema} -> Result<Value, List<Violation>>` |
| `ValidateRequired` | `{obj, fields} -> Result<Value, List<Missing>>` |

### 2.11 Normalization / repair

`NormalizePhone{value, default_region} -> Result<E164>` ·
`NormalizeEmail{value}` ·
`NormalizeUrl{value}` ·
`NormalizeWhitespace{value}` ·
`NormalizeCase{value, target}` ·
`StripHtml{value}` ·
`StripControlChars{value}` ·
`CoerceNumberFromText{value} -> Result<Num>` ("1,500" → 1500)

These are the `repair` functions referenced by tool parameter specs (§4.5).

### 2.12 Encoding & hashing

`Hash{value, algo: Sha256\|Blake3\|Xxh3} -> Str` ·
`Hmac{value, key_ref, algo} -> Str` — `key_ref`, never a raw key ·
`Base64Encode` / `Base64Decode` ·
`UrlEncode` / `UrlDecode` ·
`HexEncode` / `HexDecode` ·
`IdemKey[Str...] -> Str` — canonical ordering, stable across runs ·
`SigHash{shape: ShapeSig} -> Str` ·
`Redact{value, policy: RedactPolicy} -> Value`

### 2.13 Text metrics (deterministic, non-semantic)

`TokenCountApprox{value, model_family} -> Int` ·
`WordCount` · `CharClassProfile{value} -> {digits, alpha, symbols, spaces}` ·
`DetectCharset` · `IsAllDigits` · `IsAlphanumeric` ·
`LongestCommonPrefix[Str...]`

`CharClassProfile` is a feeder for response-signature features (§4.6).

---

## 3. S0-C — Effect & async primitives

Not pure; every call is ledger-visible.

| Op | Signature | Notes |
|---|---|---|
| `Now` | `-> Instant` | non-deterministic; recorded for replay |
| `Uuid` | `-> Id` | non-deterministic; recorded |
| `Random` | `{seed: Str?} -> Float` | seeded and ledgered, or replays diverge |
| `Park` | `{until: Instant \| Event \| Ttl} -> Suspend` | **not** blocking sleep — persists the instance, ends the turn, scheduler resumes |
| `Emit` | `{event: Event}` | internal bus |
| `Schedule` | `{at: Instant, op_ref, idem_key}` | future work |
| `Cancel` | `{schedule_id}` | |
| `LedgerAppend` | `{record} -> Receipt` | |
| `MetricInc` / `MetricObserve` | `{name, value, labels}` | |
| `TraceSpan` | `{name, body: Op}` | |

`Sleep` does not exist. In a system with turn boundaries, blocking sleep holds a worker for the entire OTP window.

---

## 4. L1 — Abilities

An ability has a cost, can fail for reasons outside its inputs, and often has more than one implementation. That last property is the defining line against S0-B.

### 4.1 Sensing — default awareness

| Ability | Signature | Sub |
|---|---|---|
| `Sense.Env` | `-> {now, tz, locale, session_age, turn_index, channel}` | `P` |
| `Sense.Budget` | `-> {tokens_left, ms_left, calls_left, cost_spent}` | `P` |
| `Sense.Self` | `-> {escalation_rate, recent_failures, degraded_abilities, cache_hit_rate}` | `P` |
| `Sense.Location` | `{consent_ref} -> Option<Geo>` | `E` |
| `Sense.Session` | `-> {open_loops, active_flow, pending_step, last_seen}` | `P` |
| `Sense.Tenant` | `-> {tenant_id, plan, feature_flags, tool_count}` | `P` |

`Sense.Self` is the health surface: rising `escalation_rate` on an ability means the semantic layer has drifted from tenant reality.

### 4.2 Understanding — utterance → structure

| Ability | Signature | Sub |
|---|---|---|
| `Understand.SplitClauses` | `{utterance} -> List<Clause>` | `L` |
| `Understand.ClassifyDepth` | `{utterance, ctx} -> {shallow\|boundary\|deep} + margin` | `S⇄L` |
| `Understand.ClassifyIntent` | `{utterance, label_set} -> Label + margin` | `S⇄L` |
| `Understand.ClassifyReplyType` | `{utterance} -> Generic\|InputOriented\|OutputOriented\|Banter` | `S⇄L` |
| `Understand.Extract` | `{utterance, schema} -> TypedRecord \| Partial{missing}` | `S⇄L` |
| `Understand.ResolveTemporal` | `{utterance, anchors} -> Interval \| Unresolved` | `S⇄L` |
| `Understand.ResolveReference` | `{utterance, candidates} -> Id \| Ambiguous[..] \| Unresolved` | `S⇄L` |
| `Understand.DetectRepair` | `{utterance, prev_turn} -> Normal\|Repair\|Frustration\|Abandon` | `S⇄L` |
| `Understand.DetectConsent` | `{utterance, proposed_action} -> Affirm\|Deny\|Unclear` | `S⇄L` |
| `Understand.Tokenize` | `{utterance, locale} -> List<Token>` | `S` |
| `Understand.DetectLanguage` | `{utterance} -> LangTag + margin` | `S` |
| `Understand.RewriteQuery` | `{utterance, ctx} -> SearchString` | `S⇄L` |

`ResolveReference` returns `Ambiguous[..]` rather than a best guess. Confident-wrong resolution is the worst failure mode in a CRM context; ambiguity is a legitimate typed outcome that triggers a clarification block.

`DetectRepair` is the cheapest honest negative signal available to the cold loop.

### 4.3 Recall — four modes, kept separate

| Ability | Signature | Sub |
|---|---|---|
| `Recall.Exact` | `{space, key} -> Option<Record>` | `P` |
| `Recall.Semantic` | `{space, query: Vector, k, filter} -> List<Hit>` | `S` |
| `Recall.Lexical` | `{space, terms, k, filter} -> List<Hit>` | `S` |
| `Recall.Graph` | `{node, axis, depth, limit} -> List<Node>` | `S` |
| `Recall.Neighbors` | `{node, edge_type, limit} -> List<Node>` | `S` |
| `Recall.Path` | `{from, to, max_hops} -> Option<Path>` | `S` |
| `Recall.Fuse` | `{sets, weights, strategy: RRF\|Weighted\|Cascade} -> List<Hit>` | `P` |
| `Recall.Rerank` | `{candidates, query} -> List<Hit>` | `S⇄L` |
| `Recall.Embed` | `{text, model_ref} -> Vector` | `S` |
| `Recall.EmbedBatch` | `{texts, model_ref} -> List<Vector>` | `S` |

`Recall.Fuse` is a deterministic combiner, not a chooser. Which combination to run is a **recall shape** (§5.1), selected deterministically from query shape.

### 4.4 Judging — the part that keeps the model honest

| Ability | Signature | Sub |
|---|---|---|
| `Judge.Confidence` | `{candidates} -> {margin, entropy, calibrated_p}` | `P` |
| `Judge.Sufficient` | `{slots, requirement} -> Complete \| Missing[..]` | `P` |
| `Judge.Postcondition` | `{state, invariant} -> Ok \| Violation` | `P` |
| `Judge.Admissible` | `{ability, ctx} -> Ok \| Denied(reason)` | `P` |
| `Judge.Verify` | `{claim, evidence} -> Bool + ReasonCode` | `L` |
| `Judge.Groundedness` | `{utterance, evidence} -> Float + unsupported_spans` | `L` |
| `Judge.Outcome` | `{turn_record} -> OutcomeScore` | `P` (from signals, not model) |

`Judge.Confidence` is first-class rather than a hidden field inside each `⇄` ability, so escalation thresholds are measurable and tunable per ability rather than being magic constants.

### 4.5 Binding & invocation

**Parameter spec** (what the SDK registers per tool field):

```
{
  name, type, required,
  constraint:  { range | pattern | enum | length | format },
  source:      user | slot | state | env | derived(expr) | tool_output(ref) | const,
  prompt_hint,
  sensitivity: none | pii | secret,
  default,
  depends_on:  [field],
  repair:      NormalizeFn
}
```

`source` is load-bearing: most arguments should never be asked for. Without it, the binder asks the user for things the system already knows — the most common way agentic systems feel stupid.

| Ability | Signature | Sub |
|---|---|---|
| `Bind.Resolve` | `{param_spec, sources} -> Value \| NeedsUser` | `P` |
| `Bind.ResolveAll` | `{tool_spec, sources} -> {bound, residual: List<Param>}` | `P` |
| `Bind.Normalize` | `{value, repair} -> Value \| Violation` | `P` |
| `Bind.Validate` | `{args, tool_spec} -> Ok \| List<Violation>` | `P` |
| `Invoke.Call` | `{tool, args, idem_key} -> RawResponse \| ToolError` | `E` |
| `Invoke.Signature` | `{raw} -> SigHash` | `P` |
| `Invoke.Extract` | `{raw, plan} -> TypedRecord \| SigMismatch` | `P` |
| `Invoke.Interpret` | `{raw, tool_spec} -> TypedRecord + proposed_plan` | `L` (cold path only) |
| `Invoke.ClassifyError` | `{tool_error, taxonomy} -> ReasonCode + Recovery` | `S⇄L` |

Only `Invoke.Call` is externally effectful. The ledger brackets `Invoke.Call` alone — that is the only line where a crash actually loses information.

### 4.6 Response signature machinery

```
ShapeSig = {
  key_set, depth, type_per_path, cardinality,
  value_features: { length, char_class, pattern_class, range_class }
}
```

| Ability | Signature | Sub |
|---|---|---|
| `Sig.Compute` | `{raw} -> ShapeSig` | `P` |
| `Sig.Hash` | `{shape} -> SigHash` | `P` |
| `Sig.Match` | `{hash, registry} -> Option<ExtractionPlan>` | `P` |
| `Sig.Propose` | `{raw, interpreted} -> ExtractionPlan` | `P` |
| `Sig.Classify` | `{raw, spec} -> Data\|EffectConfirmation\|Continuation\|Error` | `P` |

Lifecycle: unknown signature → `Invoke.Interpret` → proposal → ledger. Known and promoted → apply plan, no model, microseconds. **Mismatch → never guess**; fall back to interpret and open a new proposal branch. Cached extractor applied to a response it wasn't derived from is silent corruption, which is worse than an error.

Structure is learned. Meaning (secret / expiring / echo-to-next-call) is **declared** at registration. `{"code": "434543"}` is otherwise indistinguishable from a discount code.

### 4.7 Expression — surface generation

| Ability | Signature | Sub |
|---|---|---|
| `Express.Template` | `{id, bindings} -> Utterance` | `P` |
| `Express.Ask` | `{missing_slot, hint, attempt_n} -> Utterance` | `P⇄L` |
| `Express.Confirm` | `{action, args, personality} -> Utterance` | `P⇄L` |
| `Express.Clarify` | `{ambiguity: Ambiguous[..], personality} -> Utterance` | `P⇄L` |
| `Express.Synthesize` | `{evidence, personality, constraints} -> Utterance` | `L` |
| `Express.Apologize` | `{reason_code, recovery, personality} -> Utterance` | `P⇄L` |
| `Express.Style` | `{utterance, personality} -> Utterance` | `L` |

`Express.Synthesize` is the only open-output ability in the system, and it is terminal — nothing consumes its output but the user.

### 4.8 Persistence & memory

| Ability | Signature | Sub |
|---|---|---|
| `Remember.WriteTurn` | `{turn_record}` | `E` |
| `Remember.WriteFact` | `{fact, subject, confidence, provenance}` | `E` |
| `Remember.WriteEntity` | `{entity, attrs, aliases}` | `E` |
| `Remember.LinkEntity` | `{a, b, edge_type}` | `E` |
| `Remember.OpenLoop` | `{loop_spec} -> LoopId` | `E` |
| `Remember.CloseLoop` | `{loop_id, resolution}` | `E` |
| `Remember.Forget` | `{selector, reason}` | `E` |
| `Remember.Decay` | `{policy}` | `E` |
| `Remember.Consolidate` | `{window} -> Summary` | `L` |

### 4.9 State & policy

| Ability | Signature | Sub |
|---|---|---|
| `State.Read` | `{user} -> StateNode` | `P` |
| `State.ProposeTransition` | `{user, target, evidence} -> Ok \| Rejected(reason)` | `E` |
| `State.Reachable` | `{from} -> List<StateNode>` | `P` |
| `State.Direction` | `{current} -> Option<TargetState>` | `P` |
| `Policy.Evaluate` | `{subject, action, ctx} -> Allow \| Deny(reason)` | `P` |
| `Policy.Explain` | `{decision} -> List<PredicateTrace>` | `P` |

`Policy.Evaluate` is never a model. It is enforced as an **executor invariant** wrapping every `Invoke.Call` and `State.ProposeTransition` — structurally, not composed in, so the composer cannot forget it.

### 4.10 Registry & discovery

`Registry.LookupTool{tenant, selector} -> List<ToolSpec>` ·
`Registry.LookupFlow{tenant, selector} -> List<FlowSpec>` ·
`Registry.LookupProcedure{tenant, situation_key} -> List<ProcedureSpec>` ·
`Registry.Capabilities{tenant} -> List<CapabilityTag>` ·
`Registry.Reachable{tenant, state, policy_ctx} -> List<AbilityRef>` ·
`Registry.Version{ref} -> Version` ·
`Registry.Invalidate{tool_id} -> List<InvalidatedRef>` — the `tool_deps` cascade

### 4.11 Learning

| Ability | Signature | Sub |
|---|---|---|
| `Learn.SituationKey` | `{state, intent, slots, tools, sig} -> Vector` | `P` |
| `Learn.LookupTier` | `{σ} -> Tier0\|Tier1\|Tier2\|Tier3 + candidates` | `P` |
| `Learn.Compose` | `{goal: required_evidence, available_slots, capability_envelope, allow_effects, bounds; procedure_graph} -> Option<Path>` | `P` — bounded recursive backward type/evidence search |
| `Learn.ProposePath` | `{σ, ability_set} -> Path` | `L` — cold start only, constrained to declared abilities |
| `Learn.TypeCheck` | `{path} -> Ok \| Unsatisfiable(step)` | `P` |
| `Learn.ScoreStep` | `{step_record} -> StepScore` | `P` |
| `Learn.Attribute` | `{path_record, outcome} -> List<StepCredit>` | `P` |
| `Learn.Propose` | `{candidate} -> ProposalId` | `E` |
| `Learn.Promote` | `{proposal} -> Ok \| GateNotMet(reason)` | `E` |
| `Learn.Demote` | `{ref, reason}` | `E` |
| `Learn.Explore` | `{σ, budget} -> Option<AlternatePath>` | `P` |

`Learn.ProposePath` never emits prose. It selects an ordered path over the declared ability set, type-checked before a single step executes.

### 4.12 Proactive / outbound

`Proactive.EvaluateLoops{user} -> List<Candidate>` ·
`Proactive.Decide{candidates, policy_table} -> Option<Action>` — deterministic table, gates before any model ·
`Proactive.Cadence{user} -> {allowed_window, jitter}` ·
`Proactive.BudgetCheck{user, action} -> Ok \| Exhausted` ·
`Proactive.Send{action, idem_key} -> Receipt` ·
`Proactive.Suppress{user, reason, ttl}`

### 4.13 Observability

`Observe.Trace{turn_id} -> TurnTrace` ·
`Observe.Explain{decision} -> Explanation` ·
`Observe.PendingProposals{tenant} -> List<Proposal>` ·
`Observe.Approve{proposal_id, actor}` ·
`Observe.Metrics{tenant, window} -> MetricSet`

---

## 5. L2 — Blocks

Fixed compositions authored once by you, not per tenant. Each has the standard contract and is indistinguishable from a primitive to its caller.

### 5.1 Recall shapes

A small closed set, selected deterministically from query shape — not chosen by a model.

| Block | Composition | Selected when |
|---|---|---|
| `AnchorThenTraverse` | `Recall.Exact → Recall.Graph → Recall.Fuse` | named entity present, known axis |
| `FilterThenRank` | `Recall.Lexical(filter) → Recall.Semantic → Recall.Fuse` | hard filters + fuzzy intent |
| `LexicalConfirm` | `Recall.Semantic → Recall.Lexical(verify) → Recall.Fuse` | exact-token requirement (ids, codes) |
| `GraphOnly` | `Recall.Graph` | pure relationship query |
| `TemporalWindow` | `Understand.ResolveTemporal → Recall.Exact(range) → Recall.Semantic` | temporal scope present |
| `HybridBroad` | all four → `Recall.Fuse(RRF)` | cold / unclassifiable |

### 5.2 Extraction & repair

| Block | Composition |
|---|---|
| `ExtractValidateRepair` | `Understand.Extract → Bind.Normalize → Bind.Validate → Loop(Express.Ask, max_iter=3)` |
| `CollectSlot` | `Bind.Resolve → Branch(NeedsUser → Express.Ask → Park)` |
| `CollectAllSlots` | `Bind.ResolveAll → ForEach(residual → CollectSlot)` |
| `DisambiguateRef` | `Understand.ResolveReference → Branch(Ambiguous → Express.Clarify → Park)` |

### 5.3 Tool call lifecycle

```
ToolCallBlock =
  Guard(Policy.Evaluate)
  → Bind.ResolveAll → CollectAllSlots → Bind.Validate
  → IdemKey → Tee(LedgerAppend: intent)
  → Once(Invoke.Call)
  → Tee(LedgerAppend: receipt)
  → Sig.Compute → Sig.Hash → Sig.Match
  → Branch(hit  → Invoke.Extract
           miss → Invoke.Interpret → Sig.Propose → Learn.Propose)
  → Sig.Classify
  → Switch(Data → slots.write
           EffectConfirmation → state evidence
           Continuation → enqueue expected next
           Error → Invoke.ClassifyError → Recovery)
```

Related: `ToolCallWithRetry` = `Retry(ToolCallBlock, policy)` — admissible only for idempotent tools.

### 5.4 Message triage

```
TriageBlock =
  Sense.Env, Sense.Session
  → Understand.ClassifyDepth (S, escalate on low margin)
  → Switch(
      shallow  → ShallowAnswer
      boundary → BoundaryAnswer
      deep     → DeepTurn)
```

- `ShallowAnswer` = `Understand.ClassifyReplyType → Express.Template` (generic + state-answerable; no model after cold start)
- `BoundaryAnswer` = `Sense.* → Express.Synthesize` (single LLM call, no tools)
- `DeepTurn` = the full executor

### 5.5 Turn spine

```
TurnBlock =
  Sense.Env / Budget / Session
  → Understand.SplitClauses
  → ForEach(clause →
       TriageBlock
       → Learn.SituationKey
       → Learn.LookupTier
       → Switch(Tier0/1 → execute promoted path
                Tier2   → Learn.Compose → Learn.TypeCheck → execute
                Tier3   → Learn.ProposePath → Learn.TypeCheck → execute supervised))
  → Judge.Sufficient
  → Express.Synthesize
  → Tee(Remember.WriteTurn, LedgerAppend)
```

### 5.6 Flow control

`ActivateFlow` = hard preconditions → `Policy.Evaluate` → situation match + margin → activate
`AdvanceFlow` = load instance → resolve step → satisfy via block or promoted procedure → `Judge.Postcondition` → advance
`SuspendFlow` = persist slots + pending step + TTL + attempts → `Park`
`ResumeFlow` = load → **re-check policy and state** → continue
`DeviateFlow` = `Switch(fallback | free-range | escalate)` per flow's declared escape

Resumed flows must never trust pre-suspension authorization.

### 5.7 Confirmation & safety

`ConfirmBeforeEffect` = `Express.Confirm → Park → Understand.DetectConsent → Branch(Affirm → proceed, Deny → abort, Unclear → re-ask ≤2)`
`DegradeGracefully` = `Fallback(full → reduced → template → handoff)`
`HumanHandoff` = `Remember.OpenLoop → Proactive.Suppress → Express.Template`

### 5.8 Cold loop blocks

`ScoreTurn` = `Judge.Outcome ← {tool ok, repair detected, flow terminal, abandonment, latency, cost}`
`AttributeCredit` = `ForEach(step → Learn.ScoreStep) → Learn.Attribute`
`PromotionSweep` = `ForEach(proposal → Learn.Promote | hold)`
`InvalidationCascade` = `Registry.Invalidate → ForEach(dependent → Learn.Demote)`
`ExplorationSchedule` = sample tier-0 hits at rate ε → `Learn.Explore` → run runner-up → record comparative outcome

---

## 6. L3 — Procedures (learned)

Not enumerable — this is the schema, and the vocabulary grows at runtime.

```
ProcedureSpec {
  id, version, tenant_id,
  situation_key: Vector,
  situation_filter: { state, intent_class, required_slots, capability_tags },
  path: Composition,                  // over L1/L2/L3
  contract: AbilityContract,          // §9.1
  tool_deps: [ToolId],
  evidence: { observations, success_rate, mean_cost, mean_latency,
              last_success, last_failure },
  status: proposed | candidate | promoted | suspended | retired,
  provenance: { origin: tier2|tier3, proposed_by, approved_by? },
  supersedes: Option<ProcedureId>
}
```

Rules:
- **Immutable once promoted.** Improvements create v2 alongside v1; both retrievable, ranked by evidence. Gives rollback when a "better" path loses under real traffic.
- **Asymmetric gating.** Slow to promote (≥N clean observations clearing success-rate and cost thresholds), instant to suspend on mismatch or dependency change. Cheap to re-earn, expensive to be wrong.
- **Per-step credit.** A six-step failure demotes the failing step, not the surrounding five.

---

## 7. L4 — Flows (authored rails)

```
FlowSpec {
  id, version, tenant_id, name,
  activation: {
    hard_preconditions: [Predicate],     // state, policy — evaluated first
    trigger_surface: [Utterance|Vector], // embedding match breaks ties only
    margin_threshold: Float
  },
  learnable: Bool,                       // false for auth/payment/destructive
  preemption: Hold | SuspendYield | Yield,
  steps: [ FlowStep ],
  escape: Fallback(flow) | FreeRange | Escalate,
  terminal_states: [StateRef],
  ttl, max_attempts
}

FlowStep {
  id, intent,
  postcondition: Invariant,   // WHAT must be true — not HOW
  admissible: [AbilityRef|Capability],
  on_violation: Repair | Escape,
  suspendable: Bool
}
```

The **seam**: any L2 block or promoted L3 procedure satisfying a step's `postcondition` is admissible. The flow is a contract about *sequence*, not implementation — so learning improves flows from underneath while ordering stays frozen. Without `postcondition`, that seam is unsound.

Flows with `learnable: false` are structurally barred from reordering proposals — not merely disinclined. Learning may still tune extraction thresholds, repair wording, and retry timing beneath a frozen sequence.

---

## 8. L5 — Lifecycle, policy, personality

```
StateSpec {
  id, name,
  permission_envelope: [Capability],   // what is reachable from here
  direction: Option<{ target: StateRef, nudge_policy }>,
  entry_conditions: [Predicate],
  exit_edges: [{ to, guard: Predicate, evidence_required }],
  timeout: Option<{ after, to }>
}

PolicySpec {
  id, effect: Allow | Deny,
  subject:  { role, state, tenant, segment },
  action:   { capability, tool_id, transition, flow_id },
  condition: Predicate,                // CLOSED grammar — see below
  reason_code, priority
}
```

**Policy predicate grammar (closed).** Predicates may reference only: `state`, `role`, `slot`, `env` (time/locale/channel), `tool`, `consent`, `budget`, `evidence`, `flow_context`. Operators: comparison, membership, range, `and`/`or`/`not`. No function calls, no loops, no I/O. Closing this grammar is what prevents policy from becoming tenant-supplied code you execute.

```
PersonalitySpec {
  id, tenant_id, segment?,
  voice: { register, verbosity, formality, emoji_policy },
  lexicon: { preferred, forbidden },
  constraints: [ HardConstraint ],       // never promise, never quote price, ...
  templates: Map<TemplateId, Str>,
  fallback_style
}
```

Personality constrains `Express.*` only. It must never enter retrieval scoring or policy evaluation — otherwise tone changes silently alter what the system can find and do.

---

## 9. Cross-cutting

### 9.1 The uniform contract (every rung, L0→L4)

```
Ability {
  id, version,
  in: TypedSchema,  out: TypedSchema,  fail: [ReasonCode],
  substrate: Pure|Semantic|Llm|Effect,
  cost_class: Free|Cheap|Moderate|Expensive,
  idempotent: Bool,  effectful: Bool,  suspendable: Bool,
  preconditions:  [Predicate],
  postconditions: [Invariant],
  situation_key:  Option<Vector>,
  tool_deps: [ToolId],
  sensitivity: none|pii|secret
}
```

### 9.2 ReasonCode taxonomy (closed, top level)

`Validation` · `Missing` · `Ambiguous` · `Denied` · `NotFound` · `Conflict` ·
`RateLimited` · `Timeout` · `Unavailable` · `ToolError` · `SigMismatch` ·
`BudgetExceeded` · `LoopBudgetExceeded` · `Unsatisfiable` · `Suspended` ·
`Cancelled` · `Internal`

Each carries `recovery: Retryable | NeedsRepair | Terminal | NeedsEscalation`. Error signatures are as learnable as success signatures and are what turn the executor from brittle into resilient.

### 9.3 Cost classes

| Class | Budget | Examples |
|---|---|---|
| `Free` | ~0 | all S0-B |
| `Cheap` | <10ms | embedding lookup, vector search, exact recall |
| `Moderate` | <500ms | tool call, graph traversal |
| `Expensive` | seconds + tokens | any `L` substrate ability |

Composition ordering is cost-ascending: cheap deterministic filters run first, model calls last and only on residual uncertainty.

### 9.4 Determinism budget per turn (target)

| Tier | LLM calls | Notes |
|---|---|---|
| shallow | 0 | template + state |
| boundary | 1 | synthesis only |
| deep, tier 0/1 | 1 | terminal synthesis only |
| deep, tier 2 | 1–2 | synthesis + possible extract escalation |
| deep, tier 3 | 3–5 | cold start; should trend toward 0 share of traffic |

Share of traffic served at tier 0/1 is the headline efficiency metric for the whole system.

---

## 10. Known gaps

Enumerated so they are not mistaken for oversights.

1. **Memory block taxonomy** — episodic / factual / entity / procedural / open-loop each need distinct write rules, decay, and retrieval shape. Entity identity resolution across turns is the hard sub-problem.
2. **Evidence & outcome signal set** — `Judge.Outcome` currently takes signals not fully enumerated. Without this the promotion gate has no input and the learning loop is decorative.
3. **Nudge / direction semantics** — the `direction` half of `StateSpec` is specified structurally but its policy is unwritten.
4. **Cross-tenant generalization** — default is no procedure sharing across tenants. Genuine strategic question, not settled.
5. **Calibration procedure** — every `⇄` pair needs an empirical threshold per ability per tenant; the calibration harness is undesigned.
6. **Multi-clause conflict** — when `SplitClauses` yields clauses whose resolutions contradict, resolution order is unspecified.
7. **Exploration rate ε** — value, decay schedule, and per-flow opt-out undecided.
8. **Locale/currency formatting** — needed for CRM tenants; belongs as an ability (needs locale data), not a pure op.
