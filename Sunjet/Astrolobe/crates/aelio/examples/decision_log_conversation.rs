//! Run a small conversation and print a readable Decision Log for every turn.
//!
//! ```text
//! # scripted (no paid LLM, fully deterministic):
//! cargo run -p aelio --example decision_log_conversation
//!
//! # live through the TypeScript LLM gateway:
//! AELIO_LLM_GATEWAY_URL=http://localhost:8787/v1/llm/complete \
//!   cargo run -p aelio --example decision_log_conversation
//! ```
//!
//! Each turn prints what every hot-loop stage decided and *why*, so you can read the machine's
//! reasoning turn-by-turn and judge whether it is behaving. The renderer independently redacts
//! inputs, replies, and trace details, so it remains safe even if an upstream step regresses.

use std::fmt::Write as _;
use std::path::PathBuf;

use aelio::decision_log;
use aelio::provider::{GatewayEmbedder, TsGatewayProvider};
use aelio::World;

const USER: &str = "u1";

/// The scripted conversation. Each line drives one turn; the runtime threads state and any parked
/// flow between turns on its own.
const SCRIPT: &[&str] = &[
    "hi",
    "what can you do?",
    "log in",
    "9876543210", // phone → normalized, send_otp fires, flow parks awaiting the code
    "111111",     // wrong code → verify_otp returns invalid_otp (watch the reason code + repair)
    "434543",     // correct code → transition to authenticated
    // Same normalized, read-only request accumulates evidence: Tier-2 → promote → Tier-0.
    "give me the ten most avaricious clients",
    "give me the ten most avaricious clients",
    "give me the ten most avaricious clients",
    "give me the ten most avaricious clients",
];

fn main() {
    let mut world = World::demo_tenant("decision-log-tenant");

    // Live path: dial the TS gateway when configured; otherwise stay fully deterministic.
    let mode = match std::env::var("AELIO_LLM_GATEWAY_URL") {
        Ok(url) => {
            match TsGatewayProvider::from_env() {
                Ok(provider) => {
                    world.set_llm_provider(Box::new(provider));
                    format!("live gateway → {url}")
                }
                Err(error) => {
                    eprintln!("[warn] gateway configured but unavailable ({error}); using scripted provider");
                    "scripted (gateway init failed)".to_string()
                }
            }
        }
        Err(_) => "scripted (deterministic, 0 paid calls)".to_string(),
    };

    // Route embeddings through the TS gateway too (OpenAI/Anthropic/Gemini by key), so σ near-match,
    // intent, and term bridging use a real semantic space instead of the offline bag-of-hash default.
    let embed_mode = match GatewayEmbedder::from_env() {
        Ok(embedder) => {
            world.set_embedder(Box::new(embedder));
            "gateway embeddings (real model)"
        }
        Err(_) => "bag-of-hash embeddings (offline default)",
    };

    println!("Aelio decision log — provider: {mode} | {embed_mode}\n");

    let mut transcript = format!("Aelio decision log — provider: {mode}\n\n");
    let mut total_llm = 0u32;

    for (turn_no, utterance) in SCRIPT.iter().enumerate() {
        let state_before = world
            .user_state
            .get(USER)
            .cloned()
            .unwrap_or_else(|| "unauthenticated".into());
        // The runtime assigns turn_index = count of this user's prior turns, i.e. the loop index.
        let turn_no = turn_no as u64;

        let result = world.run_turn(USER, utterance);
        total_llm += result.llm_calls;

        let block = decision_log::render(turn_no, USER, utterance, &state_before, &result);
        println!("{block}");
        let _ = writeln!(transcript, "{block}");
    }

    let summary = format!(
        "SUMMARY  turns={}  total_llm_calls={}  final_state={}\n",
        SCRIPT.len(),
        total_llm,
        world
            .user_state
            .get(USER)
            .cloned()
            .unwrap_or_else(|| "unauthenticated".into()),
    );
    println!("{summary}");
    transcript.push_str(&summary);

    if let Some(path) = write_transcript(&transcript) {
        println!("Wrote decision log → {}", path.display());
    }
}

/// Best-effort: persist the full transcript under the repo's decision_logs directory.
fn write_transcript(transcript: &str) -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR = <repo>/Sunjet/Astrolobe/crates/aelio → climb to <repo>.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)?
        .to_path_buf();
    let dir = repo.join("docs/new_arch/decision_logs");
    std::fs::create_dir_all(&dir).ok()?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!(
        "conversation_{timestamp}_{}.log",
        std::process::id()
    ));
    std::fs::write(&path, transcript).ok()?;
    Some(path)
}
