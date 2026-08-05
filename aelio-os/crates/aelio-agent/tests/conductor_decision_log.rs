//! Turn-by-turn Conductor decision log (rule-first selection).
//! Run: cargo test -p aelio-agent --test conductor_decision_log -- --nocapture

use aelio_agent::runtime::World;
use aelio_agent::{
    detect_stack_control, select_starter_harness, HarnessPlayMode, HarnessSession, StackControl,
    StarterHarness, CONDUCTOR_ID,
};

fn explain_stack_control(utterance: &str) -> (StackControl, String) {
    let control = detect_stack_control(utterance);
    let text = utterance.trim().to_lowercase();
    let why = match control {
        StackControl::Fresh => {
            let hit = [
                "start over",
                "start again",
                "start fresh",
                "forget everything",
                "new topic",
                "reset",
            ]
            .iter()
            .find(|p| text.contains(*p))
            .unwrap_or(&"fresh-phrase");
            format!("matched Fresh phrase `{hit}`")
        }
        StackControl::ExitUp => {
            let hit = ["go back", "go up", "never mind", "exit this", "cancel", "back"]
                .iter()
                .find(|p| text.contains(*p) || text.trim() == **p)
                .unwrap_or(&"exit-phrase");
            format!("matched ExitUp phrase `{hit}`")
        }
        StackControl::Stay => "no fresh/exit-up phrase → Stay".into(),
    };
    (control, why)
}

/// Mirror `select_starter_harness` with an audit trail of which rule fired.
fn explain_select(utterance: &str, session: &HarnessSession) -> (StarterHarness, Vec<String>) {
    let mut log = Vec::new();
    let text = utterance.trim().to_lowercase();
    log.push(format!("normalize utterance → {text:?}"));

    if text.is_empty() {
        log.push("RULE hit: empty → QuickReply".into());
        return (StarterHarness::QuickReply, log);
    }

    let sc = detect_stack_control(utterance);
    if matches!(sc, StackControl::Fresh | StackControl::ExitUp) {
        log.push(format!(
            "RULE hit: stack_control={sc:?} → QuickReply (after clear/pop)"
        ));
        return (StarterHarness::QuickReply, log);
    }
    log.push(format!("check stack_control={sc:?} → not Fresh/ExitUp"));

    if text.contains("wait")
        || text.contains("hold on")
        || text.contains("what do you need")
        || text.contains("ask me")
    {
        log.push(
            "RULE hit: wait/clarify cues (wait|hold on|what do you need|ask me) → WaitForUser"
                .into(),
        );
        return (StarterHarness::WaitForUser, log);
    }
    log.push("check wait/clarify cues → no".into());

    if text.contains("remember")
        || text.contains("recall")
        || text.contains("what did i say")
        || text.contains("from earlier")
    {
        log.push("RULE hit: memory cues → MemoryAttach".into());
        return (StarterHarness::MemoryAttach, log);
    }
    log.push("check memory cues → no".into());

    const EFFECT_HINTS: &[&str] = &[
        "send otp",
        "login",
        "log in",
        "verify otp",
        "create job",
        "list jobs",
        "delete",
        "cancel",
        "pay",
        "book",
    ];
    if let Some(h) = EFFECT_HINTS.iter().find(|h| text.contains(*h)) {
        log.push(format!(
            "RULE hit: effect hint `{h}` → Escalate (cold ProposePath)"
        ));
        return (StarterHarness::Escalate, log);
    }
    log.push("check effect hints (otp/login/jobs/…) → no".into());

    let deep = text.len() > 80
        || (text.contains('?')
            && (text.contains("how")
                || text.contains("why")
                || text.contains("which")
                || text.contains("should")))
        || text.contains("help me")
        || text.contains("i want to")
        || text.contains("i need to");
    if deep {
        log.push(format!(
            "RULE hit: deep/ambiguous (len={} help_me/want/need/?+how|why…) → UnderstandIntent",
            text.len()
        ));
        return (StarterHarness::UnderstandIntent, log);
    }
    log.push(format!(
        "check deep/ambiguous (len={}, help_me={}, ?) → no",
        text.len(),
        text.contains("help me")
    ));

    let words = text.split_whitespace().count();
    let greeting = words <= 4
        || ["hi", "hello", "hey", "thanks", "thank you", "ok", "okay", "yes", "no"]
            .iter()
            .any(|w| text == *w || text.starts_with(&format!("{w} ")));
    if greeting {
        log.push(format!(
            "RULE hit: tiny/greeting (words={words}) → QuickReply"
        ));
        return (StarterHarness::QuickReply, log);
    }
    log.push(format!("check tiny/greeting (words={words}) → no"));

    if session
        .pages
        .get(CONDUCTOR_ID)
        .is_some_and(|page| page.notes.len() >= 2)
    {
        log.push("RULE hit: conductor page has ≥2 notes → QuickReply".into());
        return (StarterHarness::QuickReply, log);
    }
    log.push("check rich conductor notes → no".into());

    log.push("RULE default → UnderstandIntent".into());
    (StarterHarness::UnderstandIntent, log)
}

#[test]
fn log_conductor_decisions_turn_by_turn() {
    let script = [
        "hello",
        "help me figure out what to do next with my hiring pipeline",
        "please ask me what you need",
        "I need a short summary of open roles",
        "start over",
        "send otp to 9611266596",
    ];

    let mut world = World::demo_tenant("decision-log");
    world.set_harness_play_mode(HarnessPlayMode::Stored);

    println!("\n========== CONDUCTOR DECISION LOG (rule-first) ==========\n");
    println!("Important: Conductor does NOT embed/search the library by meaning yet.");
    println!("It runs ordered IF rules in select_starter_harness, then loads that id from catalog.\n");

    for (i, utterance) in script.iter().enumerate() {
        let session_before = world
            .user_harness
            .get("u1")
            .cloned()
            .unwrap_or_default();
        let top = session_before
            .top()
            .map(|f| format!("{} waiting={}", f.harness_id, f.waiting))
            .unwrap_or_else(|| "(empty stack)".into());
        let waiting = session_before
            .waiting_child()
            .map(|f| f.harness_id.clone());

        println!("################ TURN {i} ################");
        println!("USER: {utterance:?}");
        println!("STACK before: top={top}");
        if let Some(w) = &waiting {
            println!("WAITING CHILD owns next message unless ExitUp/Fresh → `{w}`");
        }

        let (control, control_why) = explain_stack_control(utterance);
        println!("1) STACK CONTROL: {control:?}  ({control_why})");

        let abandon = matches!(control, StackControl::Fresh | StackControl::ExitUp);
        if !abandon {
            if let Some(w) = &waiting {
                if w == "wait_for_user" {
                    println!("2) SELECTION: SKIPPED — waiting child `{w}` resumes inline");
                    println!("   (Conductor does not re-choose a harness this turn)");
                    let result = world.run_turn("u1", utterance);
                    println!("3) ACTUAL TRACE:");
                    for s in &result.steps {
                        if s.name.starts_with("Harness")
                            || s.name.starts_with("Conductor")
                            || s.name == "ProposePath"
                        {
                            println!("   · {} :: {}", s.name, s.detail);
                        }
                    }
                    println!("REPLY: {}\n", result.reply.text.replace('\n', " | "));
                    continue;
                }
            }
        } else {
            println!("2) waiting child abandoned due to {control:?}");
        }

        // Selection against post-control session (approximate: Fresh clears pages).
        let mut preview = session_before.clone();
        match control {
            StackControl::Fresh => preview.fresh(true),
            StackControl::ExitUp => {
                let _ = preview.exit_up();
            }
            StackControl::Stay => {}
        }
        let (choice, rules) = explain_select(utterance, &preview);
        let actual = select_starter_harness(utterance, &preview);
        assert_eq!(choice, actual, "audit trail must match real selector");

        println!("2) CONDUCTOR SELECT (ordered rules):");
        for line in &rules {
            println!("   - {line}");
        }
        println!(
            "   => CHOSEN: {} — {}",
            choice.id(),
            choice.description()
        );

        if choice == StarterHarness::Escalate {
            println!("3) RETRIEVE: none (Escalate → cold ProposePath, not a library program)");
        } else {
            let prog = world
                .tenant
                .harness_programs
                .get(choice.id())
                .expect("starter must be in catalog");
            println!(
                "3) RETRIEVE from catalog harness_programs[{}]: version={} hash={}",
                prog.id,
                prog.version,
                prog.content_hash()
            );
            println!("   description: {}", prog.description);
            println!("   steps:");
            for step in &prog.steps {
                println!("     • {step:?}");
            }
        }

        let result = world.run_turn("u1", utterance);
        println!("4) ACTUAL RUNTIME TRACE:");
        for s in &result.steps {
            if s.name.starts_with("Harness")
                || s.name.starts_with("Conductor")
                || s.name == "ProposePath"
            {
                println!("   · {} :: {}", s.name, s.detail);
            }
        }
        let top_after = world.user_harness["u1"]
            .top()
            .map(|f| format!("{} waiting={}", f.harness_id, f.waiting))
            .unwrap_or_else(|| "(empty)".into());
        println!("STACK after: {top_after}");
        println!(
            "REPLY (suspended={}): {}\n",
            result.suspended,
            result.reply.text.replace('\n', " | ")
        );
    }

    println!("========== END DECISION LOG ==========\n");
}
