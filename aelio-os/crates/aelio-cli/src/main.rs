//! aelio CLI (§26) — `compile`, `run`, `replay`, `trace`, Flow Forge, Mint.
//!
//! ```text
//! aelio compile <plan.json>
//! aelio run   --plan plan.json [--bag bag.json] [--ledger-out ledger.json]
//! aelio replay --plan plan.json --ledger ledger.json [--bag bag.json]
//! aelio trace --plan plan.json [--bag bag.json]
//! aelio forge --prompt "..." [--tenant demo] [--mock|--openai] [--store DIR] [--out flow.json]
//! aelio mint  --objective "..." --slot name:str!pii --out field:type [--store DIR] [--id id]
//! aelio mint-recall --tenant TENANT --need "..." --store DIR
//! ```

use aelio_kernel::{compile, replay, Instance, Ledger, Registry, TurnOutcome};
use aelio_prompt::{MintRequest, MintSlot};
use aelio_runtime::{
    forge_flow, mint_prompt, open_mint_shelf, recall_minted, ForgeRequest, MockDrafter,
    MockMintDrafter, Runtime, RuntimeConfig,
};
use aelio_sol::{structural_imprint, value_hash, SolValue};
use std::collections::BTreeMap;
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
        return Err(
            "usage: aelio <compile|run|replay|trace|hash|imprint|forge|mint|mint-recall> …".into(),
        );
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
        "imprint" => {
            let path = args.first().ok_or("usage: aelio imprint <value.json>")?;
            let value = load_bag(Some(path))?;
            println!("{}", structural_imprint(&value));
        }
        "forge" => {
            run_forge(&args)?;
        }
        "mint" => {
            run_mint(&args)?;
        }
        "mint-recall" => {
            run_mint_recall(&args)?;
        }
        "run" => {
            let plan_path = required_flag(&args, "--plan")?;
            let bag_path = flag(&args, "--bag");
            let ledger_out = flag(&args, "--ledger-out");
            let program = compile(&read(plan_path)?).map_err(display_kernel)?;
            let bag = load_bag(bag_path)?;
            let mut reg = Registry::default();
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
            let plan_path = required_flag(&args, "--plan")?;
            let bag_path = flag(&args, "--bag");
            let program = compile(&read(plan_path)?).map_err(display_kernel)?;
            let bag = load_bag(bag_path)?;
            let mut reg = Registry::default();
            let _ = &mut reg;
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

fn run_forge(args: &[String]) -> Result<(), String> {
    let prompt = flag(args, "--prompt").ok_or(
        "usage: aelio forge --prompt \"...\" [--tenant demo] [--openai] [--store DIR] [--out flow.json] [--flow-id id]",
    )?;
    let tenant = flag(args, "--tenant").unwrap_or("default");
    let flow_id = flag(args, "--flow-id").map(str::to_string);
    let flow_rev = flag(args, "--flow-rev").unwrap_or("1");
    let out = flag(args, "--out");
    let store_dir = flag(args, "--store");
    let use_openai = args.iter().any(|a| a == "--openai");
    let store = store_dir.is_some();

    let request = ForgeRequest {
        tenant: tenant.into(),
        prompt: prompt.into(),
        flow_id,
        flow_rev: flow_rev.into(),
        store,
    };

    let runtime = if let Some(dir) = store_dir {
        Some(
            Runtime::open(RuntimeConfig {
                data_dir: dir.into(),
                host_url: std::env::var("AELIO_HOST_URL").ok(),
                host_token: std::env::var("AELIO_HOST_TOKEN").ok(),
                event_key_secret: std::env::var("AELIO_EVENT_KEY_SECRET")
                    .ok()
                    .and_then(|value| {
                        let bytes = value.as_bytes();
                        if bytes.len() == 32 {
                            let mut secret = [0u8; 32];
                            secret.copy_from_slice(bytes);
                            Some(secret)
                        } else {
                            None
                        }
                    })
                    .unwrap_or([9u8; 32]),
                queue_depth: 8,
            })
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };

    let result = if use_openai {
        runtime
            .as_ref()
            .ok_or("live forge requires --store DIR so the authoritative runtime can be opened")?
            .forge_flow_with_host(&request)
            .map_err(|error| error.to_string())?
    } else {
        forge_flow(runtime.as_ref(), &request, &MockDrafter).map_err(|error| error.to_string())?
    };

    println!(
        "accepted={} stored={} drafter={} flow_id={}@{} summary={}",
        result.accepted,
        result.stored,
        result.drafter,
        result.flow.flow_id,
        result.flow.flow_rev,
        result.summary
    );
    let pretty = serde_json::to_string_pretty(&result.flow)
        .map_err(|error| format!("serialize flow: {error}"))?;
    if let Some(path) = out {
        fs::write(path, &pretty).map_err(|error| format!("cannot write `{path}`: {error}"))?;
        println!("wrote {path}");
    } else {
        println!("{pretty}");
    }
    Ok(())
}

fn run_mint(args: &[String]) -> Result<(), String> {
    let _ = flag(args, "--objective").or(flag(args, "--request")).ok_or(
        "usage: aelio mint --objective \"...\" --slot name:type[!|?][sens] --out field:type [--id id] [--tenant demo] [--store DIR] [--openai] [--request file.json] [--write file.json]",
    )?;
    let req = if let Some(path) = flag(args, "--request") {
        serde_json::from_str(&read(path)?).map_err(|e| format!("invalid mint request JSON: {e}"))?
    } else {
        let objective = flag(args, "--objective").unwrap();
        let tenant = flag(args, "--tenant").unwrap_or("default");
        let id = flag(args, "--id").map(str::to_string);
        let version = flag(args, "--version").unwrap_or("1");
        let system_input = parse_slots(args)?;
        let output = parse_output_fields(args)?;
        let input = flag(args, "--input")
            .map(|s| serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({ "note": s })));
        MintRequest {
            tenant: tenant.into(),
            objective: objective.into(),
            system_input,
            output,
            input,
            id,
            version: version.into(),
        }
    };
    finish_mint(args, req)
}

fn finish_mint(args: &[String], req: MintRequest) -> Result<(), String> {
    let store_dir = flag(args, "--store");
    let out = flag(args, "--write");
    let use_openai = args.iter().any(|a| a == "--openai");
    let result = if use_openai {
        let dir = store_dir
            .ok_or("live mint requires --store DIR so the authoritative runtime can be opened")?;
        let runtime = Runtime::open(RuntimeConfig {
            data_dir: dir.into(),
            host_url: std::env::var("AELIO_HOST_URL").ok(),
            host_token: std::env::var("AELIO_HOST_TOKEN").ok(),
            event_key_secret: [9; 32],
            queue_depth: 8,
        })
        .map_err(|error| error.to_string())?;
        runtime
            .mint_prompt_with_host(&req, true)
            .map_err(|error| error.to_string())?
    } else {
        let shelf = if let Some(dir) = store_dir {
            Some(open_mint_shelf(dir).map_err(|e| e.to_string())?)
        } else {
            None
        };
        mint_prompt(shelf.as_ref(), &req, &MockMintDrafter).map_err(|e| e.to_string())?
    };
    println!(
        "accepted={} stored={} drafter={} key={} imprint={}",
        result.accepted,
        result.stored,
        result.drafter,
        result.artifact.key(),
        result.artifact.output_imprint
    );
    let pretty = serde_json::to_string_pretty(&result.artifact)
        .map_err(|e| format!("serialize artifact: {e}"))?;
    if let Some(path) = out {
        fs::write(path, &pretty).map_err(|e| format!("cannot write `{path}`: {e}"))?;
        println!("wrote {path}");
    } else {
        println!("{pretty}");
    }
    Ok(())
}

fn run_mint_recall(args: &[String]) -> Result<(), String> {
    let need = flag(args, "--need")
        .ok_or("usage: aelio mint-recall --tenant TENANT --need \"...\" --store DIR")?;
    let tenant = flag(args, "--tenant").ok_or("--tenant is required for mint-recall")?;
    let store_dir = flag(args, "--store").ok_or("--store DIR is required for mint-recall")?;
    let shelf = open_mint_shelf(store_dir).map_err(|e| e.to_string())?;
    let hits = recall_minted(&shelf, tenant, need).map_err(|e| e.to_string())?;
    println!("hits={}", hits.len());
    for hit in hits {
        let id = hit.fields.get("id");
        let ver = hit.fields.get("version");
        let desc = hit.fields.get("description");
        println!(
            "score={:.4} id={id:?} version={ver:?} description={desc:?}",
            hit.score
        );
    }
    Ok(())
}

/// `--slot name:str!pii`  `!`=required `?`=optional; sensitivity optional after.
fn parse_slots(args: &[String]) -> Result<Vec<MintSlot>, String> {
    let mut slots = Vec::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if args[i] == "--slot" {
            slots.push(parse_one_slot(&args[i + 1])?);
            i += 2;
            continue;
        }
        i += 1;
    }
    if slots.is_empty() {
        return Err("at least one --slot name:type is required".into());
    }
    Ok(slots)
}

fn parse_one_slot(spec: &str) -> Result<MintSlot, String> {
    let (name, rest) = spec
        .split_once(':')
        .ok_or_else(|| format!("slot `{spec}` must be name:type"))?;
    let (ty, required, sensitivity) = if let Some((ty, sens)) = rest.split_once('!') {
        (ty, true, if sens.is_empty() { "public" } else { sens })
    } else if let Some((ty, sens)) = rest.split_once('?') {
        (ty, false, if sens.is_empty() { "public" } else { sens })
    } else {
        (rest, true, "public")
    };
    Ok(MintSlot {
        name: name.into(),
        ty: ty.into(),
        required,
        sensitivity: sensitivity.into(),
    })
}

fn parse_output_fields(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if args[i] == "--out" {
            let spec = &args[i + 1];
            let (name, ty) = spec
                .split_once(':')
                .ok_or_else(|| format!("--out `{spec}` must be field:type"))?;
            out.insert(name.into(), ty.into());
            i += 2;
            continue;
        }
        i += 1;
    }
    if out.is_empty() {
        return Err("at least one --out field:type is required".into());
    }
    Ok(out)
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
