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
    if let Err(error) = run() {
        eprintln!("error: {error}");
        exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return Err("usage: aelio <compile|run|replay|trace|hash> …".into());
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "compile" => {
            let path = args.first().ok_or("usage: aelio compile <plan.json>")?;
            let text = read(path)?;
            match compile(&text) {
                Ok(node) => {
                    println!("ok nid={} (planner passed)", node.nid);
                }
                Err(e) => {
                    return Err(format!("reject {} — {}", e.code.code(), e.detail));
                }
            }
        }
        "hash" => {
            let path = args.first().ok_or("usage: aelio hash <bag.json>")?;
            let bag = load_bag(Some(path))?;
            println!("{}", value_hash(&bag));
        }
        "run" => {
            let plan_path = required_flag(&args, "--plan")?;
            let bag_path = flag(&args, "--bag");
            let ledger_out = flag(&args, "--ledger-out");
            let program = compile(&read(plan_path)?).map_err(display_kernel)?;
            let bag = load_bag(bag_path)?;
            let mut reg = Registry::default();
            // Identity registry: any Call fails closed unless registered — demo uses pure Const/Seq.
            let _ = &mut reg;
            let mut instance = Instance::new(program, &mut reg);
            match instance.start(bag) {
                Ok(TurnOutcome::Completed { bag_hash, .. }) => {
                    println!("completed bag_hash={bag_hash}");
                    print_ledger(instance.ledger());
                    write_ledger(ledger_out, instance.ledger())?;
                }
                Ok(TurnOutcome::Parked(p)) => {
                    println!("parked at {}", p.park_nid);
                    print_ledger(instance.ledger());
                    write_ledger(ledger_out, instance.ledger())?;
                }
                Err(e) => {
                    write_ledger(ledger_out, instance.ledger())?;
                    return Err(display_kernel(e));
                }
            }
        }
        "trace" => {
            // Run a (pure) plan and print the §26 op-by-op trace reconstructed from the ledger.
            let plan_path = required_flag(&args, "--plan")?;
            let bag_path = flag(&args, "--bag");
            let program = compile(&read(plan_path)?).map_err(display_kernel)?;
            let bag = load_bag(bag_path)?;
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
                    return Err(display_kernel(e));
                }
            }
        }
        "replay" => {
            let plan_path = required_flag(&args, "--plan")?;
            let ledger_path = required_flag(&args, "--ledger")?;
            let bag_path = flag(&args, "--bag");
            let program = compile(&read(plan_path)?).map_err(display_kernel)?;
            let bag = load_bag(bag_path)?;
            let ledger = Ledger::from_json_str(&read(ledger_path)?)
                .map_err(|error| format!("invalid ledger: {error}"))?;
            let bag_hash = replay(&program, &ledger, bag).map_err(display_kernel)?;
            println!("replay bit-identical bag_hash={bag_hash}");
            println!("ledger_entries={}", ledger.entries().len());
            ledger
                .verify_chain()
                .map_err(|seq| format!("ledger chain failed at seq {seq}"))?;
            println!("ledger_chain=ok");
        }
        other => {
            return Err(format!("unknown command `{other}`"));
        }
    }
    Ok(())
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn required_flag<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    flag(args, name).ok_or_else(|| format!("required flag `{name}` is missing"))
}

fn read(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("cannot read `{path}`: {error}"))
}

fn display_kernel(error: aelio_kernel::ErrV1) -> String {
    format!("{} — {}", error.code.code(), error.detail)
}

fn load_bag(path: Option<&str>) -> Result<SolValue, String> {
    match path {
        None => Ok(SolValue::map::<_, &str>([])),
        Some(p) => {
            let j: serde_json::Value = serde_json::from_str(&read(p)?)
                .map_err(|error| format!("invalid bag JSON `{p}`: {error}"))?;
            aelio_kernel::json_from(&j).map_err(|error| format!("invalid Sol bag `{p}`: {error}"))
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

fn write_ledger(path: Option<&str>, ledger: &Ledger) -> Result<(), String> {
    if let Some(path) = path {
        let json = ledger
            .to_json_pretty()
            .map_err(|error| format!("cannot serialize ledger: {error}"))?;
        fs::write(path, json).map_err(|error| format!("cannot write ledger `{path}`: {error}"))?;
    }
    Ok(())
}
