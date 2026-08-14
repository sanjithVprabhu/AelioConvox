# Aelio Conversational Architecture — Repo Family

## Why this exists

Aelio gives any product an agentic, conversational interface. Every product that adopts it puts a user through roughly the same lifecycle, first contact, verification, learning about the user, discovering what the product can do, taking a first meaningful action, then using it regularly. Right now, if ten teams build on Aelio, ten teams will each invent their own greeting copy, their own verification flow, their own onboarding questions, from scratch.

The fix is a family of toolkit repos, one per lifecycle stage, each containing the schemas, conversational content, and UI specs a product builder needs to assemble that stage without designing it themselves. A team picks the repos relevant to their product and configures them, they don't rebuild them.

## The lifecycle

| # | Stage | What has to happen | Scope |
|---|---|---|---|
| 1 | Un-Verified | Greet the user and pitch what the product does for them, in under 100 characters | Per user (happens once) |
| 2 | Verified | Confirm the user is who they say they are | Per user (happens once) |
| 3 | Onboarding | Collect information about the user, conversationally | Per user (happens once, though individual attributes can be collected progressively over time) |
| 4 | Feature Discovery | Introduce a specific feature's capabilities | Per user, per feature |
| 5 | Feature Activation | Get the user to take a first real action in a specific feature | Per user, per feature |
| 6 | Power Usage | Sustain regular, repeated use of a specific feature | Per user, per feature |

Stages 1 through 3 happen once for a user and apply to the whole product. Stages 4 through 6 are not global, they run independently for every feature a user encounters. A user can be a power user of feature A, mid-discovery on feature B (which just shipped), and never have touched feature C, all at the same time. This is a firm architectural decision, not an open question: `aelio-discover`, `aelio-activate`, and `aelio-retain` are all built around a per-`(user, feature)` state, never a single per-user value.

### Data model: per-feature lifecycle state

Every product using these three repos needs one state record per `(user, feature)` pair, not a single lifecycle field on the user record. Roughly:

```
user_id: 123
feature_states:
  - feature_id: resume_builder
    stage: power_usage
    entered_stage_at: 2026-05-02
    entry_method: system_triggered
  - feature_id: interview_prep
    stage: feature_discovery
    entered_stage_at: 2026-08-10
    entry_method: user_invoked
  - feature_id: job_alerts
    stage: not_started
```

The `stage` field has a fixed enum, in order: `not_started` → `feature_discovery` → `activated` → `power_usage`. This ordering matters beyond bookkeeping, other features can gate their own discovery on a specific stage being reached for a different feature, not just on discovery having happened, see `aelio-discover` below.

The `entry_method` field records whether the feature was reached because the product's sequencing surfaced it (`system_triggered`) or because the user asked for it directly (`user_invoked`). A direct ask can move a feature straight from `not_started` into `feature_discovery` and often `activated` in the same turn, bypassing whatever `stage_reached` or `event` trigger would normally gate it, users are always allowed to pull a feature themselves. See the `user_invoked` trigger type under `aelio-discover`.

A new feature shipping to the product doesn't touch any existing `feature_states` entries, it just adds a new row, in `not_started`, for every user. This is what lets `aelio-discover` surface the new feature to a long-time power user of other features without disturbing their state anywhere else.

## The shared pattern every repo follows

Every repo in this family, regardless of which stage it covers, is built the same way, so a product builder learns the pattern once and reuses it everywhere:

- **Schema**: a meta-schema defining the shape of an artifact in this stage (what fields it has, what's required).
- **Content library**: the actual conversational copy, prompts, and phrasings, written to be asked naturally rather than presented as a form label.
- **UI format**: for every piece of content, a specific interface component it should render as (quick-reply chips, a dropdown, an OAuth button, a native permission prompt, and so on), plus a worked example of the response shape. This is what stops every team from re-deciding "should this be a text box or a dropdown" from scratch.
- **Manifest**: a config file a product builder fills in to select which artifacts from the repo they actually want active for their product, nothing is force-included.

Below, each stage's repo is described using this same four-part shape.

## 1. `aelio-greet` — Un-Verified stage

**Purpose**: the first thing an unauthenticated user sees. A greeting plus a benefit-oriented pitch of what the product does, kept under 100 characters so it reads like a message, not a landing page.

- **Schema**: greeting definition, fields for `product_context` (what vertical this is for), `char_limit` (100), `tone` (formal, casual, playful), and an optional `cta` (what happens if the user engages, e.g. "Get Started").
- **Content library**: pre-written greeting templates by vertical. Example for a career agent: *"Hi, I'm Dev. I'll help you land your next role faster by matching your resume to real openings."* (94 characters).
- **UI format**: a chat bubble or welcome card, paired with a single quick-reply CTA ("Let's go" / "Tell me more").
- **Manifest**: which greeting variant fires, based on the product's vertical and acquisition channel (a user arriving from a job board ad might see a different pitch than one arriving from a generic app store listing).

## 2. `aelio-verify` — Verified stage

**Purpose**: confirm the user's identity, and in doing so, capture whatever contact channel or credential that confirmation depends on.

**The boundary principle, and why some fields moved here from intake**: if collecting a piece of information is the verification mechanism itself, it belongs in this repo, not in onboarding. You can't ask for an email address without immediately using it to send a magic link or OTP, and you can't "link a social account" without that link being an OAuth verification event. Those aren't onboarding questions that happen to come early, they're the verification step. `email`, `phone_number`, `linked_social_accounts`, and `two_factor_phone` were originally listed as `aelio-intake` attributes and have moved here, since asking for them and verifying them is one continuous action, not two separate ones. Onboarding, by design, starts only once identity is already established.

**Repo structure**:

```
aelio-verify/
  schema/
    meta-schema.yaml         # same convention as aelio-intake's meta-schema
  methods/
    email-otp.yaml
    email-magic-link.yaml
    sms-otp.yaml
    oauth-google.yaml
    oauth-apple.yaml
    passkey.yaml
  attributes/
    verified_email.yaml      # moved from aelio-intake's `email`
    verified_phone.yaml      # moved from aelio-intake's `phone_number` and `two_factor_phone`, same field serves both purposes
    linked_identity_provider.yaml   # moved from aelio-intake's `linked_social_accounts`
  content/
    ask-copy/                # initial "what's your email/phone" prompts
    retry-copy/
    confirmation-copy/
  manifests/
    manifest-schema.yaml
    example-product.yaml
  tests/
    validate-methods.*
  docs/
    README.md
```

- **Schema**: two layers. `methods/` defines each verification mechanism's mechanics, `method` type (email_otp, magic_link, sms_otp, oauth, passkey), `timeout`, `max_attempts`, `fallback_method`. `attributes/` defines the actual contact/credential data captured, using the same meta-schema convention as `aelio-intake` (`ui_format`, `sensitivity_tier`, and so on), so these are proper attribute files, not one-off form fields bolted onto the verification flow.
- **Content library**: the initial ask copy ("What's your email?" / "What's your phone number?"), retry copy, and success/failure confirmations, per method.
- **UI format**:

  | Attribute | UI format | Response example |
  |---|---|---|
  | `verified_email` | Single-line text input, email keyboard, inline format validation, followed by a "check your email" state or a 6-digit OTP sent to that address | `{"email": "tej@example.com", "verified": true, "method": "magic_link"}` |
  | `verified_phone` | Country code dropdown (flag + dial code) plus number field, phone keyboard, followed by auto-advancing 6-digit OTP boxes | `{"country_code": "+1", "number": "4155551234", "verified": true}` |
  | `linked_identity_provider` | OAuth buttons ("Continue with Google" / "Continue with Apple" / etc.), no text entry, identity capture and verification happen in the same tap | `{"provider": "google", "provider_user_id": "1029384756", "verified": true}` |
  | `passkey` | Native OS passkey prompt | `{"verified": true, "method": "passkey"}` |

- **Manifest**: which methods a given product supports and in what priority order (e.g. try SSO first, fall back to email OTP), and which of `verified_email` / `verified_phone` the product actually needs as a result. If a feature later needs two-factor authentication, it reuses `verified_phone` rather than asking for a phone number a second time in onboarding, that reuse is the entire point of consolidating these fields here.

## 3. `aelio-intake` — Onboarding stage

**Purpose**: collect structured information about the user, conversationally, rather than through a static form. It's a schema of user attributes plus a library of how to elicit each one, with consent rules attached. This is the most built-out repo in the family so far, and the template the other five copy.

**Boundary with `aelio-verify`**: by the time onboarding starts, `verified_email` and/or `verified_phone` already exist on the user record, courtesy of `aelio-verify`. Don't re-ask for them here. A legitimate onboarding-stage question referencing that data is fine, e.g. "We'll send updates to `{{verified_email}}`, want to add a backup contact?", re-collecting the same email from scratch is not. `email`, `phone_number`, `linked_social_accounts`, and `two_factor_phone` have been removed from the attribute catalog below for this reason, see the `aelio-verify` section above. `username` stays here as a self-chosen display handle, but if a product's auth model uses username-plus-password as the actual login credential rather than email/phone/OAuth, that specific capture step belongs in `aelio-verify` instead, since it becomes a credential, not a profile field.

**Repo structure**:

```
aelio-intake/
  schema/
    meta-schema.yaml       # defines the shape every attribute definition must follow
  attributes/
    core/                  # universal: identity, location, preferences (contact-channel and
                            # security attributes live in aelio-verify, not here)
    saas/                  # vertical extension: company, role, team size, billing
    consumer/              # vertical extension: interests, social handles
    career/                # vertical extension: work history, skills, resume
    (add verticals as needed)
  prompts/
    core/                  # conversational elicitation templates, one file per attribute
  consent/
    sensitivity-tiers.yaml
    purpose-limitation.yaml
  mappings/
    example-platform.yaml  # shows how an external platform's user fields map onto this schema
  manifests/
    manifest-schema.yaml
    example-career-agent.yaml
  tests/
    validate-schema.*      # validates every attribute file against meta-schema
    validate-manifest.*    # validates manifests against manifest-schema and consent rules
  docs/
    README.md
    CONTRIBUTING.md
```

- **Schema**: every attribute in `/attributes` conforms to `schema/meta-schema.yaml`. Fields: `id`, `label`, `category` (identity, contact, location, security, preferences, professional, financial, health, social, behavioral), `data_type`, `enum_values`, `sensitivity_tier` (public, pii, sensitive_regulated), `source` (explicit_ask, inferred, third_party_auth), `dependency` (optional condition for when to ask it, e.g. `context == "b2b"`), `validation`, `default_prompt_ref`, `ui_format` (component type plus config), `derived_from`, and `computation` (the latter two only used for computed attributes). Rule: if an attribute can be reliably computed from a more fundamental one, mark it `derived_from` and give it no prompt and no `ui_format`, its `source` is always `inferred`. Age from date of birth is the canonical case, never ask "age" directly, ask date of birth and compute age. The same pattern applies wherever it fits (e.g. don't ask "years of experience," ask start dates and compute it).
- **Content library**: 2 to 3 natural phrasings per attribute in `/prompts/core/`, plus a graceful fallback if the user declines to answer. The prompt text and the `ui_format` must agree exactly, if the prompt asks "What's your gender?" the quick-reply labels must be exactly "Male", "Female", "Non-binary", "Prefer not to say", not a rephrased version.
- **UI format**: every core attribute has an assigned component and a worked response example, fixed so downstream platforms don't each invent their own:

  | Attribute | UI format | Response example |
  |---|---|---|
  | `name` | Two text inputs: first name, last name | `{"first_name": "Tej", "last_name": "Gowda"}` |
  | `date_of_birth` | Three dropdown selectors: Day (1–31), Month (Jan–Dec), Year (descending, current year minus 13 down to current year minus 100). Never ask "age" directly. | `{"day": 14, "month": "March", "year": 1996}` |
  | `age` | *(no UI, derived from `date_of_birth`)* | `{"age": 30}` |
  | `gender` | Quick reply chips, single select: "Male", "Female", "Non-binary", "Prefer not to say" | `{"gender": "Non-binary"}` |
  | `profile_photo` | Image upload ("Take Photo" / "Choose from Library" / "Skip"), circular crop preview | `{"profile_photo_url": "https://cdn.aelio.io/users/xxxx/avatar.jpg"}` |
  | `username` | Text input with live availability check, auto-suggests alternatives if taken | `{"username": "tej_gowda"}` |
  | `city` | Autocomplete/typeahead place lookup, not free text | `{"city": "San Francisco", "place_id": "ChIJIQBpAG2ahYAR_6128GcTUEo"}` |
  | `country` | Searchable dropdown, ISO 3166 list with flags, pre-selected from device locale or IP | `{"country": "United States", "iso_code": "US"}` |
  | `precise_location` | Native OS location permission prompt, never typed, falls back to manual `city` entry if denied | `{"lat": 37.7749, "lng": -122.4194, "accuracy_m": 20}` |
  | `language` | Searchable dropdown, pre-selected from device locale, overridable | `{"language": "English (US)", "code": "en-US"}` |
  | `notification_settings` | Toggle switch per channel (Push, Email, SMS) | `{"push": true, "email": true, "sms": false}` |
  | `interests` | Multi-select chip grid, tap to toggle, searchable if list grows large | `{"interests": ["Fitness", "Technology", "Travel"]}` |

- **Consent layer**: `consent/sensitivity-tiers.yaml` defines, per tier (public, pii, sensitive_regulated), whether the value can be inferred without asking, whether it needs explicit consent, and its retention rule. Every attribute's `sensitivity_tier` wires into this, so nothing gets collected without a reason attached.
- **Mapping layer**: `/mappings/` shows how an external platform's existing user fields (e.g. their `org_size`) map onto this canonical schema (`company_size`), the template every new platform integration follows.
- **Manifest**: a product builder selects which attributes they need, not by browsing the full catalog, but by starting from a vertical template and customizing. Each manifest entry has `attribute_id`, `tier` (`required` or `progressive`), `purpose` (required, no purpose means the attribute can't be added, this is what enforces purpose limitation), and `elicitation_trigger` (for `progressive` entries, the context that should prompt the ask). Default to `progressive`, keep `required` short, every attribute marked required is friction every user pays upfront. The validator should flag (not block) manifests where `required` makes up too large a share of the total.

## 4. `aelio-discover` — Feature Discovery stage

**Purpose**: introduce one specific feature to one specific user, when that user's `feature_states` entry for it is `not_started`. There is no single "discovery" experience for a product, there's a discovery experience per feature, evaluated independently for every user against their own state record.

**Why `discovery_priority` alone isn't enough**: ordering discoverable features by priority only decides which one shows first among features that are all equally eligible right now. It says nothing about whether a feature is eligible yet. Some features genuinely depend on the user having done something in another feature first, not just having seen it. Surfacing `interview_prep`'s discovery card to a user who has seen `resume_builder`'s card but never actually uploaded a resume is showing them a feature they can't meaningfully use yet. That's a dependency on state, not a display-order preference, so it needs its own explicit trigger, not a tie-breaker field.

- **Schema**: feature descriptor, keyed by `feature_id`, with `category`, `discovery_priority` (tie-breaker only, used when two or more features are already eligible for the same user at the same time), and `trigger` (see below, this is what determines eligibility, `discovery_priority` never does).

- **Trigger**: a `trigger` object per feature, evaluated against that user's `feature_states`. Four trigger types:

  | Trigger type | Fields | Example |
  |---|---|---|
  | `immediate` | none | `job_alerts` becomes eligible as soon as onboarding completes, no dependency |
  | `stage_reached` | `depends_on_feature` (a `feature_id`), `min_stage` (`activated` or `power_usage`), `dependency_type` (`soft` or `hard`) | `interview_prep` requires `resume_builder` to reach `activated`, not just `feature_discovery`. Being shown the resume builder's card isn't enough, they have to have actually built a resume. |
  | `event` | `event_name` (a specific usage event, independent of any other feature's stage) | `salary_insights` requires the event `viewed_3_job_postings`, which isn't tied to another feature's lifecycle at all |
  | `user_invoked` | none, always available on every feature by default | The user directly asks for a feature by name or intent ("help me prep for an interview"), regardless of what stage the product's system-driven sequencing would otherwise be in |

  Triggers can combine conditions with `all_of` / `any_of` where a feature depends on more than one thing (e.g. `interview_prep` might require both `resume_builder` at `activated` and the event `applied_to_a_job`). A feature only enters `feature_discovery` once its trigger evaluates true, `discovery_priority` then decides ordering among whichever features are eligible at that moment. Firing discovery for one feature reads other features' state to evaluate its trigger, but never writes to them.

  **Push vs. pull, and why `user_invoked` overrides the others**: `immediate`, `stage_reached`, and `event` are all system-decided, the product is choosing when to push a feature at the user. `user_invoked` is the opposite, the user is pulling a feature themselves, on their own initiative, because Aelio is a conversational interface and a user can ask for any tool at any time, not just the ones the product has decided to surface yet. A direct ask always wins over the system's sequencing plan, an agent should never tell a user "you haven't unlocked this yet." What it should do depends on `dependency_type` on that feature's `stage_reached` condition, if it has one:

  - `soft` (the default): the agent proceeds with the feature immediately, using whatever data it already has, and may mention that results would be better with the prerequisite done (e.g. "I can start prepping you for interviews now, though this'll be sharper once you've got a resume in, want to add one first or just go?"). This is the right default for most features.
  - `hard`: the feature genuinely cannot function without the prerequisite (e.g. it needs resume data to generate anything at all). In this case direct invocation should redirect into the prerequisite feature's activation flow inline, on the spot, rather than refusing the request outright ("Let's get a resume in first, then I can prep you for interviews, want to upload one now?"). Reserve `hard` for true functional blockers, not just "this would be better sequenced later," most dependencies should stay `soft`.

  A `user_invoked` entry into a feature should be recorded distinctly from a system-triggered one, see `entry_method` in the state model below, since it's a meaningfully different signal for product analytics (a user pulling a feature unprompted usually indicates stronger intent than one who clicked a suggested card).

- **Content library**: short highlight copy per feature (similar character-limit discipline to the greeting), coachmark/tooltip text, and "did you know" style nudges. Written per feature, never as a generic "here's everything we do" tour.
- **UI format**: a card carousel for browsable discovery, a contextual tooltip/coachmark for in-the-moment surfacing (e.g. highlighting `interview_prep` specifically once its trigger condition is met, independent of what stage the user is in on any other feature).
- **Manifest**: the product's actual feature list, each with its own discovery content, its `trigger` definition, and its `discovery_priority`, so both the dependency logic and the display ordering are configured per feature, not hardcoded once for the whole product.

## 5. `aelio-activate` — Feature Activation stage

**Purpose**: get one user from "aware this specific feature exists" to "took a first real action in it." Every feature has its own activation definition and its own `(user, feature)` state, a user can be activated on one feature and still pre-activation on another.

- **Schema**: activation event definition, keyed by `feature_id`, with `activation_action` (the specific thing that counts as activated, defined per feature), `success_criteria`, and `time_to_value_target`.
- **Content library**: CTA copy, a guided first-run script (step-by-step), empty-state copy for when a feature has no data yet, success confirmation copy, and a nudge for users who started but didn't finish, all written and stored per feature.
- **UI format**: an inline guided overlay or checklist for first-run flows, a progress indicator, a success toast or celebration moment on completion.
- **Trigger**: fires when a `(user, feature)` pair is in `feature_discovery` and the user engages with that feature for the first time. This includes the fast path from a `user_invoked` discovery entry, a user who asks for a feature directly can move from `not_started` to `activated` in the same conversation turn, they don't have to be walked through a system-paced discovery card first. On completion, only that feature's state record moves to `activated`, no other feature is affected, though this stage change can itself satisfy a `stage_reached` trigger for a different feature in `aelio-discover`.
- **Manifest**: defines, per feature, exactly what action counts as "activated" for that feature specifically. This matters beyond copy, it's the definition your analytics and your nudge triggers both have to agree on for that feature, so it lives here once per feature instead of being redefined inconsistently in code and in a dashboard.

## 6. `aelio-retain` — Power Usage stage

**Purpose**: sustain regular, habitual use of one specific feature once a user has activated it, and catch that specific feature's drop-off before it becomes churn. A user can be a power user of one feature while a re-engagement nudge is actively firing for another.

- **Schema**: engagement rule definition, keyed by `feature_id`, with `frequency_threshold` (what "regular use" means for this specific feature, a daily-use feature and a weekly-use feature must not share one definition), `milestone_definitions` (streaks, usage counts, per feature), and `churn_risk_signal` (what drop in activity, for that feature specifically, should trigger a re-engagement nudge).
- **Content library**: milestone/streak celebration copy, re-engagement nudge copy for users falling below their usual frequency on that feature, and expansion prompts (introducing a related feature to a user already engaged with this one), all scoped to the feature that earned them.
- **UI format**: notification templates (push, email, in-app), an in-app celebration moment for milestones, a digest/summary card for periodic recaps, each referencing the specific feature it's about.
- **Trigger**: evaluated independently per `(user, feature)` pair on a schedule or usage event, so a churn-risk nudge for `job_alerts` doesn't get triggered or suppressed by the user's activity on `resume_builder`.
- **Manifest**: the frequency thresholds and milestone rules per feature, and which nudges are allowed to fire and how often, per feature, so retention messaging for a low-frequency feature doesn't turn into notification spam calibrated for a high-frequency one.

## How the repos relate to each other

Each repo is independently usable, a single-feature product might only need `aelio-greet`, `aelio-verify`, `aelio-intake`, and `aelio-activate`, and can skip `aelio-discover` entirely since there's nothing to discover past onboarding. A product builder assembles their conversational architecture by pulling in only the repos relevant to their product and filling in each one's manifest.

All six repos share one component vocabulary (quick-reply, dropdown, OAuth button, native permission prompt, and so on) so a component defined once in `aelio-intake` isn't redefined slightly differently in `aelio-activate`. That shared vocabulary should live in a common `shared/ui-components` location referenced by all six repos rather than duplicated in each.

`aelio-discover`, `aelio-activate`, and `aelio-retain` additionally share one dependency that `aelio-greet`, `aelio-verify`, and `aelio-intake` don't need: the per-`(user, feature)` state store described above. Build this as a shared service or table those three repos read from and write to, not as three separate state trackers, otherwise a feature's discovery status and its activation status can drift out of sync with each other.

`aelio-verify` and `aelio-intake` have a similar handoff dependency to each other: `aelio-intake` reads `verified_email` and `verified_phone` off the user record that `aelio-verify` writes, rather than owning any contact-channel fields itself. If a product skips `aelio-verify` entirely (rare, but possible for low-stakes anonymous products), `aelio-intake`'s attribute catalog has no fallback for collecting an email or phone, that would need to be added back as an explicit exception, not assumed to work by default.

## Build order recommendation

Build `aelio-verify` and `aelio-intake` together first, in that order, since `aelio-intake`'s onboarding flow depends on `aelio-verify` already having written `verified_email`/`verified_phone` to the user record. `aelio-intake` is the more attribute-heavy of the two and establishes the schema and manifest pattern the other four repos copy. Then `aelio-greet` (small, and needed by every product regardless of vertical). Before starting `aelio-discover`, build the shared `(user, feature)` state store, since all three feature-specific repos depend on it. Then `aelio-discover`, `aelio-activate`, and `aelio-retain` can follow in that order, since each one's trigger logic reads the state that the previous stage writes.
