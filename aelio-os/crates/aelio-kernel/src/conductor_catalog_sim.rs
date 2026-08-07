//! Ten-harness Conductor catalog simulation (Path B decide → spawn).
//!
//! Stores `sim.*` contracts + `demo.conductor_catalog_sim`, then runs turns with
//! an injectable decide backend (oracle or LLM-shaped `ModelVerdict`).

use crate::compile;
use crate::conductor_decide::{
    register_conductor_decide, register_conductor_decide_model, DecideArgs, ModelVerdict,
    KIND_QUICK_REPLY, KIND_ROUGH_CHAT, KIND_SPAWN,
};
use crate::conductor_sol_turn::{ConductorSolTurnResult, DecideMode};
use crate::driver::{Instance, TurnOutcome};
use crate::error::{ErrV1, ReasonCode};
use crate::sol_harness_lib::{registry_with_harness_invoke, SolHarnessContract, CONTRACT_TABLE};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store};
use std::collections::HashMap;
use std::sync::Arc;

const SIM_FILES: &[(&str, &str)] = &[
    (
        "sim.weather",
        include_str!("../../../../docs/examples/sol_harness_essence/library/sim/sim_weather.json"),
    ),
    (
        "sim.calc_sum",
        include_str!("../../../../docs/examples/sol_harness_essence/library/sim/sim_calc_sum.json"),
    ),
    (
        "sim.translate",
        include_str!("../../../../docs/examples/sol_harness_essence/library/sim/sim_translate.json"),
    ),
    (
        "sim.remind",
        include_str!("../../../../docs/examples/sol_harness_essence/library/sim/sim_remind.json"),
    ),
    (
        "sim.search_docs",
        include_str!(
            "../../../../docs/examples/sol_harness_essence/library/sim/sim_search_docs.json"
        ),
    ),
    (
        "sim.order_status",
        include_str!(
            "../../../../docs/examples/sol_harness_essence/library/sim/sim_order_status.json"
        ),
    ),
    (
        "sim.joke",
        include_str!("../../../../docs/examples/sol_harness_essence/library/sim/sim_joke.json"),
    ),
    (
        "sim.unit_convert",
        include_str!(
            "../../../../docs/examples/sol_harness_essence/library/sim/sim_unit_convert.json"
        ),
    ),
    (
        "sim.calendar_next",
        include_str!(
            "../../../../docs/examples/sol_harness_essence/library/sim/sim_calendar_next.json"
        ),
    ),
    (
        "sim.email_draft",
        include_str!(
            "../../../../docs/examples/sol_harness_essence/library/sim/sim_email_draft.json"
        ),
    ),
];

const CONDUCTOR_RAW: &str = include_str!(
    "../../../../docs/examples/sol_harness_essence/library/demo_conductor_catalog_sim.json"
);

fn parse_contract(raw: &str) -> SolHarnessContract {
    let v: serde_json::Value = serde_json::from_str(raw).expect("sim json");
    let id = v["id"].as_str().expect("id").to_string();
    let summary = v["summary"].as_str().unwrap_or("").to_string();
    let program_json = v.get("program").expect("program").to_string();
    compile(&program_json).unwrap_or_else(|e| panic!("{id} must compile: {e:?}"));
    SolHarnessContract::seed(id, summary, program_json)
}

/// All ten `sim.*` contracts.
pub fn sim_catalog_contracts() -> Vec<SolHarnessContract> {
    SIM_FILES.iter().map(|(_, raw)| parse_contract(raw)).collect()
}

pub fn sim_conductor_contract() -> SolHarnessContract {
    parse_contract(CONDUCTOR_RAW)
}

/// Catalog list for `conductor.decide@1` (id + summary only).
pub fn sim_decide_catalog() -> SolValue {
    SolValue::List(
        sim_catalog_contracts()
            .into_iter()
            .map(|c| {
                SolValue::map([
                    ("id", SolValue::str(&c.id)),
                    ("summary", SolValue::str(&c.summary)),
                ])
            })
            .collect(),
    )
}

/// Persist sim harnesses + Conductor into `sol_harness_contracts`.
pub fn ensure_sim_catalog_library(store: &mut dyn Store, tenant: &str) -> Result<(), ErrV1> {
    let mut all = sim_catalog_contracts();
    all.push(sim_conductor_contract());
    for c in all {
        match store
            .put_if_absent(tenant, CONTRACT_TABLE, &c.id, c.as_store_value())
            .map_err(|e| ErrV1::new(ReasonCode::Internal, "store", format!("{e:?}")))?
        {
            PutIfAbsent::Inserted { .. } | PutIfAbsent::Existing(_) => {}
        }
    }
    Ok(())
}

fn bag_path_text(bag: &SolValue, root: &str) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get(root))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

fn decision_field(bag: &SolValue, key: &str) -> Option<String> {
    bag.as_map()
        .and_then(|m| m.get("decision"))
        .and_then(|d| d.as_map())
        .and_then(|m| m.get(key))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

/// Run one Conductor sim turn over the ten-harness catalog.
pub fn run_conductor_catalog_sim_turn(
    utterance: &str,
    mode: DecideMode,
) -> Result<ConductorSolTurnResult, ErrV1> {
    let children = sim_catalog_contracts();
    let conductor = sim_conductor_contract();
    let catalog = sim_decide_catalog();

    let mut programs = HashMap::new();
    for c in &children {
        programs.insert(c.id.clone(), c.program_json.clone());
    }

    let mut registry = registry_with_harness_invoke(programs);
    match mode {
        DecideMode::Scripted => register_conductor_decide(&mut registry),
        DecideMode::Model(classify) => register_conductor_decide_model(&mut registry, move |a| {
            classify(a)
        }),
    }

    let program = compile(&conductor.program_json)?;
    let mut inst = Instance::new(program, &mut registry);
    let bag_in = SolValue::map([
        ("utterance", SolValue::str(utterance)),
        ("catalog", catalog),
    ]);
    let (bag, bag_hash) = match inst.start(bag_in)? {
        TurnOutcome::Completed { bag, bag_hash } => (bag, bag_hash),
        TurnOutcome::Parked(_) => {
            return Err(ErrV1::new(
                ReasonCode::Internal,
                "conductor.sim",
                "demo.conductor_catalog_sim should not park",
            ))
        }
    };

    Ok(ConductorSolTurnResult {
        reply_text: bag_path_text(&bag, "out").unwrap_or_default(),
        decision_kind: decision_field(&bag, "kind").unwrap_or_default(),
        decision_source: decision_field(&bag, "source").unwrap_or_default(),
        spawned_harness_id: decision_field(&bag, "harness_id"),
        bag,
        bag_hash,
    })
}

/// Deterministic oracle for simulation chat (stand-in until live LLM is asserted).
///
/// Maps obvious utterances → catalog ids so we can verify store → decide → spawn
/// without depending on a live gateway.
pub fn sim_oracle_decide(args: &DecideArgs) -> Result<ModelVerdict, ErrV1> {
    let t = args.utterance.trim().to_lowercase();
    let spawn = |id: &str| {
        Ok(ModelVerdict {
            kind: KIND_SPAWN.into(),
            harness_id: Some(id.into()),
            confidence: 0.92,
            x: None,
            y: None,
            reply_text: None,
        })
    };
    let quick = |text: &str| {
        Ok(ModelVerdict {
            kind: KIND_QUICK_REPLY.into(),
            harness_id: None,
            confidence: 0.95,
            x: None,
            y: None,
            reply_text: Some(text.into()),
        })
    };
    let rough = |text: &str| {
        Ok(ModelVerdict {
            kind: KIND_ROUGH_CHAT.into(),
            harness_id: None,
            confidence: 0.55,
            x: None,
            y: None,
            reply_text: Some(text.into()),
        })
    };

    // Negation / preference: weather over joke when both mentioned.
    if (t.contains("don't") || t.contains("do not") || t.contains("not a joke"))
        && (t.contains("weather") || t.contains("rain") || t.contains("forecast"))
    {
        return spawn("sim.weather");
    }
    // Out-of-catalog: never invent a harness id.
    if t.contains("flight")
        || t.contains("uber")
        || t.contains("pizza")
        || t.contains("stock price")
        || t.contains("bitcoin")
    {
        return rough("I don't have a harness for that — tell me more about what you need.");
    }
    if t.contains("weather")
        || t.contains("forecast")
        || t.contains("rain")
        || t.contains("temperature")
        || t.contains("umbrella")
    {
        return spawn("sim.weather");
    }
    if t.contains("add")
        || t.contains("sum")
        || t.contains("plus")
        || (t.contains("2") && t.contains("3") && (t.contains("make") || t.contains("equal")))
    {
        return spawn("sim.calc_sum");
    }
    if t.contains("translate")
        || t.contains("spanish")
        || t.contains("french")
        || t.contains("in german")
    {
        return spawn("sim.translate");
    }
    if t.contains("remind") || t.contains("reminder") || t.contains("nudge me") {
        return spawn("sim.remind");
    }
    if t.contains("docs")
        || t.contains("documentation")
        || t.contains("help article")
        || t.contains("how do i use")
    {
        return spawn("sim.search_docs");
    }
    if t.contains("order") && (t.contains("status") || t.contains("#") || t.contains("track")) {
        return spawn("sim.order_status");
    }
    if t.contains("joke") || t.contains("funny") || t.contains("make me laugh") {
        return spawn("sim.joke");
    }
    if t.contains("convert")
        || t.contains("celsius")
        || t.contains("fahrenheit")
        || t.contains("kilometers")
        || t.contains("miles")
    {
        return spawn("sim.unit_convert");
    }
    if t.contains("calendar")
        || t.contains("schedule")
        || t.contains("next meeting")
        || t.contains("what's next today")
    {
        return spawn("sim.calendar_next");
    }
    if t.contains("email") || t.contains("draft a reply") || t.contains("write a reply") {
        return spawn("sim.email_draft");
    }
    if t.split_whitespace().count() <= 2
        && ["hi", "hello", "hey", "thanks", "ok", "bye", "thx"]
            .iter()
            .any(|w| t == *w)
    {
        return quick("Hey! I can help you.");
    }
    rough("Tell me more about what you need.")
}

/// One expected line in a simulation chat script.
#[derive(Debug, Clone)]
pub struct SimChatExpect {
    pub utterance: &'static str,
    /// Question class: direct | paraphrase | indirect | ood | greeting | chat | ambiguous | negation
    pub category: &'static str,
    pub kind: &'static str,
    pub harness_id: Option<&'static str>,
    /// Extra acceptable spawn ids (ambiguous turns). Empty ⇒ only `harness_id`.
    pub allowed_harnesses: &'static [&'static str],
    /// Extra acceptable kinds. Empty ⇒ only `kind`.
    pub allow_kinds: &'static [&'static str],
    /// Substring required in reply for exact/oracle scoring (optional for live soft scoring).
    pub reply_contains: Option<&'static str>,
}

impl SimChatExpect {
    fn spawn(
        utterance: &'static str,
        category: &'static str,
        harness_id: &'static str,
        reply_tag: &'static str,
    ) -> Self {
        Self {
            utterance,
            category,
            kind: KIND_SPAWN,
            harness_id: Some(harness_id),
            allowed_harnesses: &[],
            allow_kinds: &[],
            reply_contains: Some(reply_tag),
        }
    }

    fn reply(
        utterance: &'static str,
        category: &'static str,
        kind: &'static str,
        reply_contains: &'static str,
    ) -> Self {
        Self {
            utterance,
            category,
            kind,
            harness_id: None,
            allowed_harnesses: &[],
            allow_kinds: &[],
            reply_contains: Some(reply_contains),
        }
    }
}

/// Catalog ids the sim Conductor is allowed to spawn.
pub fn sim_catalog_ids() -> Vec<&'static str> {
    SIM_FILES.iter().map(|(id, _)| *id).collect()
}

/// Prove the Conductor's spawn actually ran (child completed and tagged the reply).
pub fn assert_sim_turn_executable(result: &ConductorSolTurnResult) -> Result<(), String> {
    let ids = sim_catalog_ids();
    match result.decision_kind.as_str() {
        KIND_SPAWN => {
            let id = result
                .spawned_harness_id
                .as_deref()
                .ok_or_else(|| "spawn missing harness_id".to_string())?;
            if !ids.iter().any(|c| *c == id) {
                return Err(format!(
                    "spawn harness_id={id} is not in the sim catalog (not executable here)"
                ));
            }
            let tag = format!("ran:{id}");
            if !result.reply_text.contains(&tag) {
                return Err(format!(
                    "spawned {id} but child did not execute — reply lacks `{tag}`: {}",
                    result.reply_text
                ));
            }
            if result.decision_source.is_empty() {
                return Err("missing decision.source".into());
            }
            Ok(())
        }
        KIND_QUICK_REPLY | KIND_ROUGH_CHAT => {
            if result.spawned_harness_id.is_some() {
                return Err("non-spawn decide must not set harness_id".into());
            }
            if result.reply_text.trim().is_empty() {
                return Err(format!(
                    "{} produced empty reply_text",
                    result.decision_kind
                ));
            }
            Ok(())
        }
        other => Err(format!("unknown decision kind `{other}`")),
    }
}

/// Whether the decide choice matches the script expectation (allows soft sets).
pub fn sim_choice_matches(expect: &SimChatExpect, result: &ConductorSolTurnResult) -> bool {
    let kinds: Vec<&str> = if expect.allow_kinds.is_empty() {
        vec![expect.kind]
    } else {
        expect.allow_kinds.to_vec()
    };
    if !kinds.iter().any(|k| *k == result.decision_kind) {
        return false;
    }
    if result.decision_kind == KIND_SPAWN {
        let mut allowed: Vec<&str> = expect.allowed_harnesses.to_vec();
        if let Some(id) = expect.harness_id {
            if !allowed.contains(&id) {
                allowed.push(id);
            }
        }
        return result
            .spawned_harness_id
            .as_deref()
            .is_some_and(|got| allowed.iter().any(|a| *a == got));
    }
    true
}

/// Grade one turn: choice + executability.
#[derive(Debug, Clone)]
pub struct SimTurnGrade {
    pub choice_ok: bool,
    pub executable_ok: bool,
    pub executable_error: Option<String>,
}

pub fn grade_sim_turn(expect: &SimChatExpect, result: &ConductorSolTurnResult) -> SimTurnGrade {
    let exec = assert_sim_turn_executable(result);
    SimTurnGrade {
        choice_ok: sim_choice_matches(expect, result),
        executable_ok: exec.is_ok(),
        executable_error: exec.err(),
    }
}

/// Core script: one direct hit per harness + greeting + chat.
pub fn sim_chat_script() -> Vec<SimChatExpect> {
    vec![
        SimChatExpect::spawn(
            "what's the weather in Paris?",
            "direct",
            "sim.weather",
            "ran:sim.weather",
        ),
        SimChatExpect::spawn(
            "please add these numbers for me",
            "direct",
            "sim.calc_sum",
            "ran:sim.calc_sum",
        ),
        SimChatExpect::spawn(
            "translate hello to Spanish",
            "direct",
            "sim.translate",
            "ran:sim.translate",
        ),
        SimChatExpect::spawn(
            "remind me tomorrow at 9 to call mom",
            "direct",
            "sim.remind",
            "ran:sim.remind",
        ),
        SimChatExpect::spawn(
            "search the docs for harness invoke",
            "direct",
            "sim.search_docs",
            "ran:sim.search_docs",
        ),
        SimChatExpect::spawn(
            "what's the status of order #4412?",
            "direct",
            "sim.order_status",
            "ran:sim.order_status",
        ),
        SimChatExpect::spawn("tell me a joke", "direct", "sim.joke", "ran:sim.joke"),
        SimChatExpect::spawn(
            "convert 100 celsius to fahrenheit",
            "direct",
            "sim.unit_convert",
            "ran:sim.unit_convert",
        ),
        SimChatExpect::spawn(
            "what's next on my calendar?",
            "direct",
            "sim.calendar_next",
            "ran:sim.calendar_next",
        ),
        SimChatExpect::spawn(
            "draft an email reply saying thanks",
            "direct",
            "sim.email_draft",
            "ran:sim.email_draft",
        ),
        SimChatExpect::reply("hi", "greeting", KIND_QUICK_REPLY, "Hey!"),
        SimChatExpect::reply(
            "I had a long day and just want to chat about it",
            "chat",
            KIND_ROUGH_CHAT,
            "Tell me more",
        ),
    ]
}

/// Broader question types: paraphrases, indirect, OOD, negation, ambiguous.
pub fn sim_chat_script_diverse() -> Vec<SimChatExpect> {
    let mut v = sim_chat_script();
    v.extend([
        // Paraphrases / synonyms
        SimChatExpect::spawn(
            "do I need an umbrella in London today?",
            "paraphrase",
            "sim.weather",
            "ran:sim.weather",
        ),
        SimChatExpect::spawn(
            "what's 2 plus 3?",
            "paraphrase",
            "sim.calc_sum",
            "ran:sim.calc_sum",
        ),
        SimChatExpect::spawn(
            "say good morning in German",
            "paraphrase",
            "sim.translate",
            "ran:sim.translate",
        ),
        SimChatExpect::spawn(
            "nudge me Friday about the dentist",
            "paraphrase",
            "sim.remind",
            "ran:sim.remind",
        ),
        SimChatExpect {
            utterance: "how do I use harness.invoke in the docs?",
            category: "paraphrase",
            kind: KIND_SPAWN,
            harness_id: Some("sim.search_docs"),
            allowed_harnesses: &["sim.search_docs"],
            // Models sometimes explain instead of spawning; still require a valid reply.
            allow_kinds: &[KIND_SPAWN, KIND_ROUGH_CHAT, KIND_QUICK_REPLY],
            reply_contains: None,
        },
        SimChatExpect::spawn(
            "track my package for order 9981",
            "paraphrase",
            "sim.order_status",
            "ran:sim.order_status",
        ),
        SimChatExpect::spawn(
            "make me laugh",
            "paraphrase",
            "sim.joke",
            "ran:sim.joke",
        ),
        SimChatExpect::spawn(
            "how many miles is 12 kilometers?",
            "paraphrase",
            "sim.unit_convert",
            "ran:sim.unit_convert",
        ),
        SimChatExpect::spawn(
            "what's next today on my schedule?",
            "paraphrase",
            "sim.calendar_next",
            "ran:sim.calendar_next",
        ),
        SimChatExpect::spawn(
            "write a reply email that apologizes for the delay",
            "paraphrase",
            "sim.email_draft",
            "ran:sim.email_draft",
        ),
        // Indirect
        SimChatExpect::spawn(
            "I'm heading out and wondering if it'll rain",
            "indirect",
            "sim.weather",
            "ran:sim.weather",
        ),
        SimChatExpect::spawn(
            "can you check whether order #12 shipped yet?",
            "indirect",
            "sim.order_status",
            "ran:sim.order_status",
        ),
        // Negation / preference
        SimChatExpect::spawn(
            "don't tell me a joke — what's the weather?",
            "negation",
            "sim.weather",
            "ran:sim.weather",
        ),
        // Out of catalog — must NOT invent a harness
        SimChatExpect {
            utterance: "book me a flight to Tokyo next Tuesday",
            category: "ood",
            kind: KIND_ROUGH_CHAT,
            harness_id: None,
            allowed_harnesses: &[],
            allow_kinds: &[KIND_ROUGH_CHAT, KIND_QUICK_REPLY],
            reply_contains: None,
        },
        SimChatExpect {
            utterance: "what's the bitcoin price right now?",
            category: "ood",
            kind: KIND_ROUGH_CHAT,
            harness_id: None,
            allowed_harnesses: &[],
            allow_kinds: &[KIND_ROUGH_CHAT, KIND_QUICK_REPLY],
            reply_contains: None,
        },
        SimChatExpect {
            utterance: "order a pizza to my house",
            category: "ood",
            kind: KIND_ROUGH_CHAT,
            harness_id: None,
            allowed_harnesses: &[],
            allow_kinds: &[KIND_ROUGH_CHAT, KIND_QUICK_REPLY],
            reply_contains: None,
        },
        // Ambiguous multi-intent — either relevant harness or chat is acceptable
        SimChatExpect {
            utterance: "will it rain tomorrow and what's on my calendar?",
            category: "ambiguous",
            kind: KIND_SPAWN,
            harness_id: Some("sim.weather"),
            allowed_harnesses: &["sim.weather", "sim.calendar_next"],
            allow_kinds: &[KIND_SPAWN, KIND_ROUGH_CHAT],
            reply_contains: None,
        },
        // Greeting variants
        SimChatExpect::reply("thanks", "greeting", KIND_QUICK_REPLY, "Hey!"),
        SimChatExpect::reply("bye", "greeting", KIND_QUICK_REPLY, "Hey!"),
        // Open chat
        SimChatExpect {
            utterance: "I've been thinking a lot about whether this job is right for me",
            category: "chat",
            kind: KIND_ROUGH_CHAT,
            harness_id: None,
            allowed_harnesses: &[],
            allow_kinds: &[KIND_ROUGH_CHAT, KIND_QUICK_REPLY],
            reply_contains: None,
        },
    ]);
    v
}

/// Run a script with the oracle decide backend.
pub fn run_sim_chat_oracle_script(
    script: &[SimChatExpect],
) -> Result<Vec<ConductorSolTurnResult>, ErrV1> {
    let classify = Arc::new(|args: &DecideArgs| sim_oracle_decide(args));
    let mode = DecideMode::Model(classify);
    let mut out = Vec::new();
    for step in script {
        out.push(run_conductor_catalog_sim_turn(step.utterance, mode.clone())?);
    }
    Ok(out)
}

/// Run the core simulation chat with the oracle decide backend.
pub fn run_sim_chat_oracle() -> Result<Vec<ConductorSolTurnResult>, ErrV1> {
    run_sim_chat_oracle_script(&sim_chat_script())
}

/// Run the diverse simulation chat with the oracle decide backend.
pub fn run_sim_chat_oracle_diverse() -> Result<Vec<ConductorSolTurnResult>, ErrV1> {
    run_sim_chat_oracle_script(&sim_chat_script_diverse())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_store::MemoryStore;
    use crate::sol_harness_lib::load_contract;

    #[test]
    fn ten_sim_harnesses_compile_and_store() {
        assert_eq!(sim_catalog_contracts().len(), 10);
        let mut store = MemoryStore::new();
        ensure_sim_catalog_library(&mut store, "sim-tenant").unwrap();
        for c in sim_catalog_contracts() {
            let loaded = load_contract(&store, "sim-tenant", &c.id)
                .unwrap()
                .expect("stored");
            assert_eq!(loaded.id, c.id);
            assert_eq!(loaded.program_json, c.program_json);
        }
        assert!(load_contract(&store, "sim-tenant", "demo.conductor_catalog_sim")
            .unwrap()
            .is_some());
    }

    #[test]
    fn simulation_chat_oracle_hits_every_harness() {
        let script = sim_chat_script();
        let results = run_sim_chat_oracle().unwrap();
        assert_eq!(results.len(), script.len());
        for (got, expect) in results.iter().zip(script.iter()) {
            let grade = grade_sim_turn(expect, got);
            assert!(
                grade.choice_ok && grade.executable_ok,
                "utterance={} grade={:?} reply={}",
                expect.utterance,
                grade,
                got.reply_text
            );
            if let Some(needle) = expect.reply_contains {
                assert!(
                    got.reply_text.contains(needle),
                    "utterance={} reply={} missing {}",
                    expect.utterance,
                    got.reply_text,
                    needle
                );
            }
        }
    }

    #[test]
    fn diverse_simulation_choices_are_executable() {
        let script = sim_chat_script_diverse();
        let results = run_sim_chat_oracle_diverse().unwrap();
        assert!(script.len() > sim_chat_script().len());
        assert_eq!(results.len(), script.len());
        for (got, expect) in results.iter().zip(script.iter()) {
            let grade = grade_sim_turn(expect, got);
            assert!(
                grade.executable_ok,
                "category={} utterance={} not executable: {:?} reply={}",
                expect.category,
                expect.utterance,
                grade.executable_error,
                got.reply_text
            );
            assert!(
                grade.choice_ok,
                "category={} utterance={} wrong choice kind={} spawn={:?} expected kind={} harness={:?}",
                expect.category,
                expect.utterance,
                got.decision_kind,
                got.spawned_harness_id,
                expect.kind,
                expect.harness_id
            );
        }
    }

    #[test]
    fn phantom_harness_spawn_is_rejected_not_executed() {
        let classify = Arc::new(|_a: &DecideArgs| {
            Ok(ModelVerdict {
                kind: KIND_SPAWN.into(),
                harness_id: Some("sim.flight_booker".into()),
                confidence: 0.99,
                x: None,
                y: None,
                reply_text: None,
            })
        });
        let err = run_conductor_catalog_sim_turn(
            "book a flight",
            DecideMode::Model(classify),
        )
        .expect_err("unknown harness must fail closed");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("not in catalog") || msg.contains("Policy") || msg.contains("sim.flight"),
            "unexpected error: {msg}"
        );
    }
}
