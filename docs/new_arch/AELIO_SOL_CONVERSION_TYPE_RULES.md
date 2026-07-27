# Sol Conversion — Fundamental Type Rules & Metadata

**Status:** draft for design lock (v0.1)  
**Companion test:** [`AELIO_SOL_CONVERSION_THEORY_TEST.md`](./AELIO_SOL_CONVERSION_THEORY_TEST.md)

Goal: every Sol→Sol converter is built from **declared rules** over **fundamental types**, with **metadata** stored in Sunjet so warm path never needs the LLM.

---

## 1. Fundamental Sol data types (`data.v`)

These are the only carriers inside `{ "k": "data", "v": … }` for Kernel-level conversion rules:

| Type id | JSON / Value | Notes |
|---|---|---|
| `null` | `null` | absence |
| `bool` | `true\|false` | |
| `int` | integer | i64 |
| `float` | number | f64 |
| `str` | string | UTF-8 |
| `list` | array | ordered; element type may be constrained |
| `map` | object | string keys; values are SolValues or nested data |

Higher tags (`var`, `fn`, `flow`, `sol`, `list` of SolValues) are **not** cast by data rules — converters either refuse or run structural rules (see §3).

---

## 2. What a conversion rule can do

Each edge is a list of **rules**. Allowed rule ops (closed set):

| `op` | Meaning | Pure? |
|---|---|---|
| `rename` | key A → key B; type unchanged | yes |
| `drop` | remove key | yes |
| `keep` | whitelist keys | yes |
| `default` | if missing, set literal | yes |
| `cast` | change fundamental type (§3) | yes if in allow-matrix |
| `wrap` | scalar → `{ "_": scalar }` or map nest under key | yes |
| `unwrap` | `{ "_": v }` → v | yes |
| `map_enum` | str→str via closed table | yes |
| `path_copy` | copy `from_path` → `to_path` | yes |
| `const_set` | set key to literal | yes |
| `propose_llm` | **not stored as warm rule** — only cold installer | no |

Warm path may only execute **pure** ops. LLM may only **propose** a list of pure ops → typecheck → promote.

---

## 3. Cast allow-matrix (fundamental → fundamental)

Legend: **Y** = always allowed · **C** = allowed with constraint · **N** = forbidden without new promoted semantic rule · **X** = never (lossy/unsafe as silent cast)

| from \ to | null | bool | int | float | str | list | map |
|---|---|---|---|---|---|---|---|
| **null** | Y | N | N | N | N | N | N |
| **bool** | N | Y | C¹ | C¹ | Y | N | N |
| **int** | N | C² | Y | Y | Y | N | N |
| **float** | N | C² | C³ | Y | Y | N | N |
| **str** | N | C⁴ | C⁵ | C⁵ | Y | N | N |
| **list** | N | N | N | N | C⁶ | Y | N |
| **map** | N | N | N | N | C⁶ | N | Y |

¹ bool→int/float: `false→0`, `true→1` only  
² int/float→bool: only `0→false`, nonzero→true if rule sets `mode: nonzero` — else **N**  
³ float→int: only if `mode: trunc|floor|ceil|round` declared; reject non-finite  
⁴ str→bool: only closed sets e.g. `{"true","false"}` / `{"0","1"}` in rule  
⁵ str→int/float: parse; fail → `ParseError` (not silent)  
⁶ list/map→str: only if `mode: json` explicit  

**Identity cast** (type→same type): always Y (no-op).

**Semantic renames** (`age`→`years_lived` same type): use `rename`, not `cast`.

**Enum / vocabulary** (`Male`→`M`): use `map_enum` with closed table — not free `cast str→str`.

---

## 4. Structural rules (non-data tags)

| Situation | Rule |
|---|---|
| Incoming `var` / `fn` / `flow` | Resolve before cast, or **refuse** (`TypeViolation`) |
| Nested `sol` | Convert inner with nested rule set, or refuse |
| `list` of maps | `scope: all` applies element-wise with same rules + `max_items` |
| Extra keys in from | `policy: drop \| keep \| error` (metadata) |
| Missing required to-key after rules | `Missing` — no invent |

---

## 5. Metadata stored per conversion edge (Sunjet)

Every promoted converter row/edge **must** carry:

### 5.1 Identity

| Field | Type | Purpose |
|---|---|---|
| `conversion_id` | str | stable id |
| `version` | str | immutable version; improvements = v2 |
| `tenant_id` | str | isolation |
| `status` | `proposed\|promoted\|suspended\|retired` | lifecycle |

### 5.2 Endpoints

| Field | Type | Purpose |
|---|---|---|
| `from_imprint` | str | source Sol imprint |
| `to_imprint` | str | target Sol imprint |
| `from_keys` | list | keys read |
| `to_keys` | list | keys written |

### 5.3 Rules & types

| Field | Type | Purpose |
|---|---|---|
| `rules` | list\<Rule\> | closed pure ops only |
| `type_map` | list `{from_key, from_type, to_key, to_type}` | fast graph search |
| `extra_key_policy` | `drop\|keep\|error` | |
| `on_parse_fail` | `error\|skip_key` | default `error` |

### 5.4 Provenance & safety

| Field | Type | Purpose |
|---|---|---|
| `proposed_by` | `llm\|human\|compose` | |
| `prompt_hash` | str? | if LLM |
| `typechecked_at` | instant | |
| `evidence` | `{observations, success_rate, last_success, last_failure}` | promote/demote |
| `sensitivity` | `none\|pii\|secret` | redact/log policy |

### 5.5 Graph index (for “find without LLM”)

Store searchable edges:

```text
(from_imprint, from_key, from_type) --> (to_imprint, to_key, to_type)
  via conversion_id
```

Lookup order:

1. Exact imprint pair + key map  
2. Field-level type edge (`age:int` → `years_lived:int`)  
3. Miss → cold LLM propose  

---

## 6. Test case 2 — `sex` → `gender` (enum map)

### Cold path

**Sol_A** (`person.raw.v1`):

```json
{
  "sol": "1",
  "imprint": "person.raw.v1",
  "body": {
    "sex": { "k": "data", "v": "Male" }
  }
}
```

**Next requires** `person.norm.v1` with `gender` in `{ "M", "F", "X", "U" }`.

Graph miss → LLM proposes:

```json
{
  "from": "person.raw.v1",
  "to": "person.norm.v1",
  "rules": [
    {
      "op": "map_enum",
      "from_key": "sex",
      "to_key": "gender",
      "from_type": "str",
      "to_type": "str",
      "table": { "Male": "M", "Female": "F", "Other": "X", "": "U" },
      "on_unknown": "error"
    }
  ]
}
```

Typecheck: `map_enum` str→str with closed table → **OK** → promote.

**Sol_B:**

```json
{
  "sol": "1",
  "imprint": "person.norm.v1",
  "body": {
    "gender": { "k": "data", "v": "M" }
  }
}
```

### Metadata row (example)

```text
conversion_id:  conv.sex_to_gender.v1
from_imprint:   person.raw.v1
to_imprint:     person.norm.v1
rules:          [ map_enum sex→gender … ]
type_map:       [{ sex:str → gender:str }]
status:         promoted
proposed_by:    llm
extra_key_policy: drop
sensitivity:    pii
```

### Warm path

Same DSL → graph HIT → no LLM → `{ gender: "M" }`.

### Failure

Input `"UnknownSex"` + `on_unknown: error` → `EnumViolation` / `ParseError` — LLM may classify repair vs hard fail on cold loop, but **must not** invent enum values in warm path.

---

## 7. Combined pipeline test (age + sex)

```text
Const({ age: 34, sex: "Male" })
  → Convert(person.raw.v1 → person.norm.v1)
       rules: rename age→years_lived, map_enum sex→gender
  → Add(2, years_lived)
  → { years_lived: 36, gender: "M" }
```

Two edges (or one composite conversion with two rules) — both pure, both metadata-complete.

---

## 8. Decisions to lock with you

1. Accept fundamental types list (`null…map`)?  
2. Accept cast matrix (especially float→int needs explicit mode)?  
3. Accept metadata fields in §5?  
4. `map_enum` mandatory for vocabulary changes (no silent str rename for Male→M)?  
5. Composite converters (many rules, one edge) vs one edge per field?

---

## 9. Verdict so far

Theory extends cleanly: **rules are typed and closed; metadata makes them findable and demotable; LLM only proposes rule lists, never runs as the warm converter.**
