//! aelio CLI (§26) — P0 surface: `compile`, `run`, `replay`.
//!
//! ```text
//! aelio compile <plan.json>
//! aelio run   --plan plan.json [--bag bag.json]
//! aelio replay --plan plan.json --ledger ledger.json [--bag bag.json]
//! aelio trace --plan plan.json [--bag bag.json]   # §26 op-by-op bag diffs from the ledger
//! ```

use aelio_kernel::{compile, replay, EffectClass, Instance, Ledger, Registry, TurnOutcome};
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
                }
                Ok(TurnOutcome::Parked(p)) => {
                    println!("parked at {}", p.park_nid);
                    print_ledger(instance.ledger());
                }
                Err(e) => {
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
                    println!("{}", aelio_kernel::trace::render_string(&program, instance.ledger()));
                    println!("completed bag_hash={bag_hash}");
                }
                Ok(TurnOutcome::Parked(p)) => {
                    println!("{}", aelio_kernel::trace::render_string(&program, instance.ledger()));
                    println!("parked at {}", p.park_nid);
                }
                Err(e) => {
                    eprintln!("error {} — {}", e.code.code(), e.detail);
                    exit(1);
                }
            }
        }
        "replay" => {
            // Minimal demo: re-run a pure plan twice and assert bag_hash identity (G2 property).
            let plan_path = flag(&args, "--plan").expect("--plan");
            let bag_path = flag(&args, "--bag");
            let text = fs::read_to_string(plan_path).unwrap();
            let program = compile(&text).expect("plan");
            let bag = load_bag(bag_path);
            let mut reg = Registry::default();
            // Register no-op targets if plan has Calls — pure plans need none.
            let _ = EffectClass::Pure;
            let mut inst = Instance::new(program.clone(), &mut reg);
            let (h1, ledger) = match inst.start(bag.clone()) {
                Ok(TurnOutcome::Completed { bag_hash, .. }) => (bag_hash, inst.ledger().clone()),
                Ok(TurnOutcome::Parked(_)) => {
                    eprintln!("replay demo requires a non-parking plan (use login golden for parks)");
                    exit(1);
                }
                Err(e) => {
                    eprintln!("{} — {}", e.code.code(), e.detail);
                    exit(1);
                }
            };
            let h2 = replay(&program, &ledger, bag).expect("replay");
            if h1 != h2 {
                eprintln!("DIVERGENCE bag_hash live={h1} replay={h2}");
                exit(1);
            }
            println!("replay bit-identical bag_hash={h1}");
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
