//! Side-by-side Hardcoded vs Stored (catalog) comparison — run with --nocapture.

use std::time::Instant;

use aelio_agent::runtime::World;
use aelio_agent::HarnessPlayMode;

fn harness_trace(result: &aelio_agent::blocks::turn::TurnResult) -> Vec<(String, String)> {
    result
        .steps
        .iter()
        .filter(|s| {
            s.name.starts_with("Conductor")
                || s.name.starts_with("Harness")
                || s.name == "ProposePath"
        })
        .map(|s| (s.name.clone(), s.detail.clone()))
        .collect()
}

#[test]
fn print_hardcoded_vs_stored_comparison() {
    let script = [
        "hello",
        "help me figure out what to do next with my hiring pipeline",
        "please ask me what you need",
        "I need a short summary of open roles",
        "start over",
        "what is 2+2 in one sentence?",
    ];

    let mut hard = World::demo_tenant("cmp-hard");
    hard.set_harness_play_mode(HarnessPlayMode::Hardcoded);
    let mut store = World::demo_tenant("cmp-store");
    store.set_harness_play_mode(HarnessPlayMode::Stored);

    println!("\n========== HARDCODED vs STORED (catalog harness_programs) ==========\n");
    println!(
        "Stored lane loads TenantDecl.harness_programs (same data persisted in Catalogs/active).\n"
    );

    let mut total_h = 0u128;
    let mut total_s = 0u128;
    let mut same_text = 0usize;
    let mut same_select = 0usize;
    let mut same_suspend = 0usize;

    for (i, utterance) in script.iter().enumerate() {
        let t0 = Instant::now();
        let rh = hard.run_turn("u1", utterance);
        let mh = t0.elapsed().as_millis();
        total_h += mh;

        let t1 = Instant::now();
        let rs = store.run_turn("u1", utterance);
        let ms = t1.elapsed().as_millis();
        total_s += ms;

        let th = harness_trace(&rh);
        let ts = harness_trace(&rs);
        let sel_h = th
            .iter()
            .find(|(n, _)| n == "Conductor.Select")
            .map(|(_, d)| d.as_str())
            .unwrap_or("(none — resume/fresh path)");
        let sel_s = ts
            .iter()
            .find(|(n, _)| n == "Conductor.Select")
            .map(|(_, d)| d.as_str())
            .unwrap_or("(none — resume/fresh path)");
        let load = ts
            .iter()
            .find(|(n, _)| n == "Harness.Load")
            .map(|(_, d)| d.as_str())
            .unwrap_or("(no load)");

        let text_same = rh.reply.text == rs.reply.text;
        let select_same = sel_h == sel_s;
        let suspend_same = rh.suspended == rs.suspended && rh.opened_loop == rs.opened_loop;
        if text_same {
            same_text += 1;
        }
        if select_same {
            same_select += 1;
        }
        if suspend_same {
            same_suspend += 1;
        }

        println!("----- turn[{i}] {utterance:?} -----");
        println!("HARDCODED  {mh}ms  suspended={} opened={}", rh.suspended, rh.opened_loop);
        println!("  select: {sel_h}");
        println!("  reply:  {}", rh.reply.text.replace('\n', " | "));
        println!("STORED     {ms}ms  suspended={} opened={}", rs.suspended, rs.opened_loop);
        println!("  select: {sel_s}");
        println!("  load:   {load}");
        println!("  reply:  {}", rs.reply.text.replace('\n', " | "));
        println!(
            "DIFF: select_same={select_same} suspend_same={suspend_same} text_same={text_same} delta_ms={:+}",
            ms as i128 - mh as i128
        );
        println!();
    }

    let n = script.len();
    println!("========== SUMMARY ==========");
    println!("turns={n}");
    println!("hardcoded total_ms={total_h}  avg_ms={}", total_h / n as u128);
    println!("stored    total_ms={total_s}  avg_ms={}", total_s / n as u128);
    println!(
        "stored_minus_hardcoded_ms={:+}  (negative => stored faster)",
        total_s as i128 - total_h as i128
    );
    println!("same_select={same_select}/{n}  same_suspend={same_suspend}/{n}  same_reply_text={same_text}/{n}");
    println!(
        "quality note: with demo ScriptedLlmProvider, reply text is often identical; control-plane parity matters most."
    );
    println!("better: neither 'wins' on quality if text matches; stored wins on architecture (save/retrieve/play).");
    println!("================================\n");

    assert_eq!(same_select, n, "selection must match every turn");
    assert_eq!(same_suspend, n, "suspend/open_loop must match every turn");
}
