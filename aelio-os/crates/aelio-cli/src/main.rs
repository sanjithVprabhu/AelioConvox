//! aelio CLI (§26) — P0 surface: `compile`, `run`, `replay`.
//!
//! ```text
//! aelio compile <plan.json>
//! aelio run   --plan plan.json [--bag bag.json] [--ledger-out ledger.json]
//! aelio replay --plan plan.json --ledger ledger.json [--bag bag.json]
//! aelio trace --plan plan.json [--bag bag.json]   # §26 op-by-op bag diffs from the ledger
//! ```

use aelio_kernel::{compile, replay, Instance, Ledger, Registry, TurnOutcome};
use aelio_sol::{value_hash, SolValue};
use std::env;
use std::fs;
use std::process::exit;

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("usage: aelio <compile|run|replay|trace|hash> …");
        exit(2);
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "compile" => {
            let path = args.first().expect("compile <plan.json>");
            let text = fs::read_to_string(path).expect("read plan");
            match compile(&text) {
                Ok(node) => {
                    println!("ok nid={} (planner passed)", node.nid);
                }
                Err(e) => {
                    eprintln!("reject {} — {}", e.code.code(), e.detail);
                    exit(1);
                }
            }
        }
        "hash" => {
            let path = args.first().expect("hash <bag.json>");
            let j: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            let bag = aelio_kernel::json_from(&j).unwrap();
            println!("{}", value_hash(&bag));
        }
        "run" => {
            let plan_path = flag(&args, "--plan").expect("--plan");
            let bag_path = flag(&args, "--bag");
            let ledger_out = flag(&args, "--ledger-out");
            let text = fs::read_to_string(plan_path).unwrap();
            let program = compile(&text).expect("plan");
            let bag = load_bag(bag_path);
            let mut reg = Registry::default();
            // Identity registry: any Call fails closed unless registered — demo uses pure Const/Seq.
            let _ = &mut reg;
            let mut instance = Instance::new(program, &mut reg);
            match instance.start(bag) {
                Ok(TurnOutcome::Completed { bag_hash, .. }) => {
                    println!("completed bag_hash={bag_hash}");
                    print_ledger(instance.ledger());
                    write_ledger(ledger_out, instance.ledger());
                }
                Ok(TurnOutcome::Parked(p)) => {
                    println!("parked at {}", p.park_nid);
                    print_ledger(instance.ledger());
                    write_ledger(ledger_out, instance.ledger());
                }
                Err(e) => {
                    write_ledger(ledger_out, instance.ledger());
                    eprintln!("error {} — {}", e.code.code(), e.detail);
                    exit(1);
                }
            }
        }
        "trace" => {
            // Run a (pure) plan and print the §26 op-by-op trace reconstructed from the ledger.
            let plan_path = flag(&args, "--plan").expect("--plan");
            let bag_path = flag(&args, "--bag");
            let text = fs::read_to_string(plan_path).unwrap();
            let program = compile(&text).expect("plan");
            let bag = load_bag(bag_path);
            let mut reg = Registry::default();
            let _ = &mut reg; // pure/Const plans need no registered targets
            let mut instance = Instance::new(program.clone(), &mut reg);
            match instance.start(bag) {
                Ok(TurnOutcome::Completed { bag_hash, .. }) => {
                    println!(
                        "{}",
                        aelio_kernel::trace::render_string(&program, instance.ledger())
                    );
                    println!("completed bag_hash={bag_hash}");
                }
                Ok(TurnOutcome::Parked(p)) => {
                    println!(
                        "{}",
                        aelio_kernel::trace::render_string(&program, instance.ledger())
                    );
                    println!("parked at {}", p.park_nid);
                }
                Err(e) => {
                    eprintln!("error {} — {}", e.code.code(), e.detail);
                    exit(1);
                }
            }
        }
        "replay" => {
            let plan_path = flag(&args, "--plan").expect("--plan");
            let ledger_path = flag(&args, "--ledger").expect("--ledger");
            let bag_path = flag(&args, "--bag");
            let text = fs::read_to_string(plan_path).unwrap();
            let program = compile(&text).expect("plan");
            let bag = load_bag(bag_path);
            let ledger_text = fs::read_to_string(ledger_path).expect("read --ledger");
            let ledger = Ledger::from_json_str(&ledger_text).expect("valid ledger");
            let bag_hash = replay(&program, &ledger, bag).expect("replay");
            println!("replay bit-identical bag_hash={bag_hash}");
            println!("ledger_entries={}", ledger.entries().len());
            ledger.verify_chain().expect("chain");
            println!("ledger_chain=ok");
        }
        other => {
            eprintln!("unknown command {other}");
            exit(2);
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn load_bag(path: Option<&str>) -> SolValue {
    match path {
        None => SolValue::map::<_, &str>([]),
        Some(p) => {
            let j: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap();
            aelio_kernel::json_from(&j).unwrap()
        }
    }
}

fn print_ledger(ledger: &Ledger) {
    for e in ledger.entries() {
        println!(
            "  seq={} kind={} nid={:?}",
            e.seq,
            e.kind,
            e.nid.as_deref().unwrap_or("-")
        );
    }
}

fn write_ledger(path: Option<&str>, ledger: &Ledger) {
    if let Some(path) = path {
        let json = ledger.to_json_pretty().expect("serialize ledger");
        fs::write(path, json).expect("write --ledger-out");
    }
}
