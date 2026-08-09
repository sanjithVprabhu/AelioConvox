//! Process-wide harness authority toggle for troubleshooting dual systems.
//!
//! There are three turn executors in this repo:
//! - **agent_loop** — provider-native model → tool → result loop
//! - **legacy / Layer B** — agent spine + hardcoded `select_starter_harness`
//! - **sol / Path B** — Sol cutovers (`conductor.root`, tool harnesses, fail-closed)
//!
//! The repository launcher selects **agent_loop** by default. Direct constructors retain
//! compatibility-safe `auto` selection unless the operator chooses a mode explicitly.
//! Operators can override with `AELIO_HARNESS_MODE`:
//! - `auto` (default) — current policy (artifact runtime ⇒ legacy)
//! - `legacy` — force Layer B agent spine
//! - `sol` — force Path B Sol cutovers (even when artifact runtime is installed)
//! - `agent_loop` — force the provider-native model/tool/result loop

/// Which harness authority is active for `/v1/turns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessMode {
    /// Respect existing flags / artifact-runtime default.
    Auto,
    /// Agent dual-IR spine + keyword Conductor.
    Legacy,
    /// Sol/OS cutovers only; do not fall through to agent spine.
    Sol,
    /// Provider-native model/tool/result loop.
    AgentLoop,
}

impl HarnessMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Legacy => "legacy",
            Self::Sol => "sol",
            Self::AgentLoop => "agent_loop",
        }
    }
}

/// Read `AELIO_HARNESS_MODE` (`agent_loop` | `auto` | `legacy` | `sol`). Unknown values ⇒ `auto`.
pub fn resolve_harness_mode() -> HarnessMode {
    let value = std::env::var("AELIO_HARNESS_MODE").ok();
    let mode = parse_harness_mode(value.as_deref());
    if mode == HarnessMode::Auto {
        if let Some(other) = value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("auto"))
        {
            eprintln!("aelio-agent-api: unknown AELIO_HARNESS_MODE={other:?}; using auto");
        }
    }
    mode
}

fn parse_harness_mode(value: Option<&str>) -> HarnessMode {
    match value
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "legacy" | "layer_b" | "layer-b" | "agent" => HarnessMode::Legacy,
        "sol" | "path_b" | "path-b" | "os" => HarnessMode::Sol,
        "agent_loop" | "agent-loop" | "loop" => HarnessMode::AgentLoop,
        "auto" | "" => HarnessMode::Auto,
        _ => HarnessMode::Auto,
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Decide whether this turn uses the legacy agent spine.
///
/// Returns `(use_legacy_spine, mode, reason)` for logs / turn traces.
pub fn resolve_legacy_spine(
    world_legacy_flow_execution_enabled: bool,
    artifact_runtime_installed: bool,
) -> (bool, HarnessMode, &'static str) {
    let mode = resolve_harness_mode();
    match mode {
        HarnessMode::Legacy => (true, mode, "AELIO_HARNESS_MODE=legacy"),
        HarnessMode::Sol => (false, mode, "AELIO_HARNESS_MODE=sol"),
        // Agent-loop turns branch before the legacy/Sol resolver is consumed. Returning false here
        // remains fail-closed if an older caller forgets that branch.
        HarnessMode::AgentLoop => (false, mode, "AELIO_HARNESS_MODE=agent_loop"),
        HarnessMode::Auto => {
            if world_legacy_flow_execution_enabled {
                (true, mode, "world.legacy_flow_execution_enabled")
            } else if env_truthy("AELIO_AGENT_LEGACY") {
                (true, mode, "AELIO_AGENT_LEGACY=1")
            } else if artifact_runtime_installed {
                // Historical production default: pinned artifact flows kept the agent spine.
                (true, mode, "artifact_runtime (auto→legacy)")
            } else {
                (false, mode, "path_b default (auto→sol)")
            }
        }
    }
}

/// One-line boot banner for the Rust runtime process.
pub fn log_harness_mode_banner(artifact_runtime_installed: bool) {
    let (legacy, mode, reason) = resolve_legacy_spine(false, artifact_runtime_installed);
    let authority = match mode {
        HarnessMode::AgentLoop => "AGENT LOOP (provider-native model/tool/result loop)",
        _ if legacy => "LEGACY agent spine (keyword Conductor / ProposePath)",
        _ => "SOL Path B (conductor.root + tool harness cutovers)",
    };
    eprintln!(
        "aelio harness authority: {authority} | mode={} | reason={reason}",
        mode.as_str()
    );
    eprintln!(
        "  toggle: AELIO_HARNESS_MODE=agent_loop|legacy|sol|auto  (also AELIO_AGENT_LEGACY=1 forces legacy in auto)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_strings() {
        assert_eq!(HarnessMode::Sol.as_str(), "sol");
        assert_eq!(HarnessMode::Legacy.as_str(), "legacy");
        assert_eq!(HarnessMode::AgentLoop.as_str(), "agent_loop");
    }

    #[test]
    fn default_cutover_and_emergency_rollback_modes_are_unambiguous() {
        assert_eq!(
            parse_harness_mode(Some("agent_loop")),
            HarnessMode::AgentLoop
        );
        assert_eq!(parse_harness_mode(Some("legacy")), HarnessMode::Legacy);
        assert_eq!(
            parse_harness_mode(Some("agent-loop")),
            HarnessMode::AgentLoop
        );
        assert_eq!(parse_harness_mode(Some("unknown")), HarnessMode::Auto);
        assert_eq!(parse_harness_mode(None), HarnessMode::Auto);
    }
}
