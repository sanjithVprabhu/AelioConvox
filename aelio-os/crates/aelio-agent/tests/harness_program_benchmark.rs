//! Stored vs hardcoded harness program benchmark + restart retrieve proof.
//!
//! Proves save → retrieve → play for HarnessProgramV1, and that stored programs
//! match hardcoded bodies on control-plane behavior (selection, suspend, stack, no ProposePath).

use std::time::Instant;

use aelio_agent::runtime::{DurableRuntime, DurableTurnRequest, World};
use aelio_agent::storage::{AelioStore, LogicalTable};
use aelio_agent::{
    quick_reply_program, starter_harness_library, HarnessPlayMode, QUICK_REPLY_ID,
};
use aelio_db_query::Database;

fn step_named(result: &aelio_agent::blocks::turn::TurnResult, name: &str) -> bool {
    result.steps.iter().any(|s| s.name == name)
}

fn selected_harness(result: &aelio_agent::blocks::turn::TurnResult) -> Option<String> {
    result.steps.iter().find_map(|s| {
        if s.name != "Conductor.Select" {
            return None;
        }
        s.detail
            .strip_prefix("harness=")
            .and_then(|rest| rest.split(" — ").next())
            .map(str::to_string)
    })
}

fn used_propose_path(result: &aelio_agent::blocks::turn::TurnResult) -> bool {
    step_named(result, "ProposePath")
}

fn reply_class(result: &aelio_agent::blocks::turn::TurnResult) -> &'static str {
    if result.suspended || result.opened_loop {
        return "ask_wait";
    }
    let text = result.reply.text.to_lowercase();
    if text.contains("hey") || text.contains("help") {
        return "greeting";
    }
    if result.reply.text.trim().is_empty() {
        return "empty";
    }
    "nonempty"
}

struct LaneOutcome {
    selected: Option<String>,
    propose: bool,
    suspended: bool,
    opened_loop: bool,
    waiting: Option<String>,
    stack_top: Option<String>,
    reply_class: &'static str,
    loaded: bool,
    ms: u128,
}

fn run_lane(mode: HarnessPlayMode, user: &str, utterances: &[&str]) -> Vec<LaneOutcome> {
    let mut world = World::demo_tenant(&format!("bench-{mode:?}"));
    world.set_harness_play_mode(mode);
    let mut out = Vec::new();
    for utterance in utterances {
        let t0 = Instant::now();
        let result = world.run_turn(user, utterance);
        let waiting = world.user_harness[user]
            .waiting_child()
            .map(|f| f.harness_id.clone());
        let stack_top = world.user_harness[user]
            .top()
            .map(|f| f.harness_id.clone());
        out.push(LaneOutcome {
            selected: selected_harness(&result),
            propose: used_propose_path(&result),
            suspended: result.suspended,
            opened_loop: result.opened_loop,
            waiting,
            stack_top,
            reply_class: reply_class(&result),
            loaded: step_named(&result, "Harness.Load"),
            ms: t0.elapsed().as_millis(),
        });
    }
    out
}

#[test]
fn stored_vs_hardcoded_control_plane_parity() {
    let script = [
        "hello",
        "help me figure out what to do next with my hiring pipeline",
        "please ask me what you need",
        "I need a short summary of open roles",
        "start over",
    ];

    let hardcoded = run_lane(HarnessPlayMode::Hardcoded, "u-hard", &script);
    let stored = run_lane(HarnessPlayMode::Stored, "u-store", &script);

    assert_eq!(hardcoded.len(), stored.len());
    for (i, (h, s)) in hardcoded.iter().zip(stored.iter()).enumerate() {
        assert_eq!(h.selected, s.selected, "utterance[{i}] selected harness");
        assert_eq!(h.propose, s.propose, "utterance[{i}] ProposePath");
        assert!(!h.propose, "utterance[{i}] must not ProposePath on demo script");
        assert_eq!(h.suspended, s.suspended, "utterance[{i}] suspended");
        assert_eq!(h.opened_loop, s.opened_loop, "utterance[{i}] opened_loop");
        assert_eq!(h.waiting, s.waiting, "utterance[{i}] waiting child");
        assert_eq!(h.stack_top, s.stack_top, "utterance[{i}] stack top");
        assert_eq!(h.reply_class, s.reply_class, "utterance[{i}] reply class");
        let is_wait_resume = h.selected.is_none()
            && script[i] != "start over"
            && i > 0
            && hardcoded[i - 1].waiting.is_some();
        assert!(
            s.loaded || s.selected.as_deref() == Some("escalate") || is_wait_resume,
            "utterance[{i}] stored lane should Harness.Load (unless escalate/resume)"
        );
        // Model-free greeting: stored must not be wildly slower.
        if i == 0 {
            assert!(
                s.ms <= h.ms.saturating_mul(5).max(50),
                "greeting latency stored={}ms hardcoded={}ms",
                s.ms,
                h.ms
            );
        }
        eprintln!(
            "[{i}] {:?} selected={:?} class={} hard={}ms store={}ms",
            script[i], s.selected, s.reply_class, h.ms, s.ms
        );
    }
}

#[test]
fn stored_quick_reply_survives_catalog_restart() {
    let path = std::env::temp_dir().join(format!(
        "aelio-harness-lib-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let expected_hash = quick_reply_program().content_hash();

    {
        let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
        let mut world = World::demo_tenant("tenant-1");
        world.tenant.harness_programs = starter_harness_library();
        let catalog = world.tenant.clone();
        let mut runtime = DurableRuntime::new(world, store).unwrap();
        runtime.register_catalog(catalog).unwrap();
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "restart-qr-1".into(),
                user_id: "u1".into(),
                utterance: "Hi".into(),
            })
            .unwrap();
        assert!(step_named(&result, "Harness.Load"));
        assert!(step_named(&result, "Harness.quick_reply") || step_named(&result, "Harness.Op"));
        assert!(!used_propose_path(&result));
        let loaded = result
            .steps
            .iter()
            .find(|s| s.name == "Harness.Load")
            .unwrap();
        assert!(
            loaded.detail.contains(&expected_hash),
            "load detail should include content hash: {}",
            loaded.detail
        );
        runtime.store.flush().unwrap();
    }

    let reopened = AelioStore::new(Database::open(&path).unwrap(), 3).unwrap();
    let record = reopened
        .get::<aelio_agent::tenant::TenantDecl>("tenant-1", LogicalTable::Catalogs, "active")
        .unwrap()
        .expect("active catalog persisted");
    let program = record
        .envelope
        .value
        .harness_programs
        .get(QUICK_REPLY_ID)
        .expect("quick_reply must be in persisted catalog");
    assert_eq!(program.content_hash(), expected_hash);

    let world = World::demo_tenant("tenant-1");
    let mut runtime = DurableRuntime::new(world, reopened).unwrap();
    // Hydrate from store replaces tenant; ensure starters still present after reload path.
    let qr = runtime
        .world
        .tenant
        .harness_programs
        .get(QUICK_REPLY_ID)
        .expect("quick_reply after DurableRuntime hydrate");
    assert_eq!(qr.content_hash(), expected_hash);

    let replay = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "restart-qr-2".into(),
            user_id: "u2".into(),
            utterance: "Hi".into(),
        })
        .unwrap();
    assert!(step_named(&replay, "Harness.Load"));
    assert!(!used_propose_path(&replay));
}
