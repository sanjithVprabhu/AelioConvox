//! Planner (§29(1)): static checks over a parsed op tree. "A plan that passes is executable by
//! construction." Parsing already enforced schema closure, literal paths, and mandatory bounds
//! (`max_iter`/`max_items`/Budget meters, App E). This pass adds the structural invariants that span
//! nodes:
//!
//! - **`sense` subtree is unwritable** (§8.4) — the reserved runtime-owned subtree; any op writing
//!   under `sense` is plan-time rejected. Load-bearing (designs out the stale-context bug class).
//! - **Park position legality** (§8.4 matrix) — Park is forbidden in `Tee.side` (a suspending side
//!   effect contradicts fire-and-record) and in predicate positions (structurally impossible: the
//!   Expr grammar has no Park, so only `Tee.side` needs a walk).
//! - **Tee dataflow isolation** (§8.2) — side writes ⊆ `side_root`.
//!
//! (Boundedness DAG acyclicity over registered flows, §8.4, is a multi-flow check deferred until the
//! registry holds flow targets — noted in FLAGS as P0.7 scope.)

use crate::error::{ErrV1, ReasonCode};
use crate::instr::{Kind, Node};
use aelio_sol::{Path, Segment};

/// Run all P0 static checks. Returns the first violation as a `Shape`/`Policy` error.
pub fn plan(root: &Node) -> Result<(), ErrV1> {
    check_writes(root)?;
    check_tee_and_park(root, false)?;
    Ok(())
}

/// Reject any write whose path is rooted at the reserved `sense` key (§8.4).
fn check_writes(node: &Node) -> Result<(), ErrV1> {
    for w in write_paths(node) {
        if is_sense(&w) {
            return Err(ErrV1::new(
                ReasonCode::Policy,
                &node.nid,
                "write under reserved runtime-owned `sense` subtree is forbidden (§8.4)",
            ));
        }
    }
    for child in children(node) {
        check_writes(child)?;
    }
    Ok(())
}

/// Park legality (§8.4 matrix). `in_tee_side` is set while walking a `Tee.side` subtree.
fn check_tee_and_park(node: &Node, in_tee_side: bool) -> Result<(), ErrV1> {
    if in_tee_side {
        if let Kind::Park { .. } = node.kind {
            return Err(ErrV1::new(
                ReasonCode::Shape,
                &node.nid,
                "Park is forbidden inside Tee.side (§8.4 matrix)",
            ));
        }
    }
    match &node.kind {
        Kind::Tee { body, side, side_root } => {
            // Side writes must stay within side_root (§8.2). Isolation from the main read set is a
            // richer check (P1); here we enforce the subtree containment.
            for w in side_writes(side) {
                if !prefix_of(side_root, &w) {
                    return Err(ErrV1::new(
                        ReasonCode::Shape,
                        &node.nid,
                        "Tee.side writes must be within side_root (§8.2)",
                    ));
                }
            }
            check_tee_and_park(body, in_tee_side)?;
            check_tee_and_park(side, true)?;
            Ok(())
        }
        _ => {
            for child in children(node) {
                check_tee_and_park(child, in_tee_side)?;
            }
            Ok(())
        }
    }
}

fn side_writes(node: &Node) -> Vec<Path> {
    let mut ws = write_paths(node);
    for child in children(node) {
        ws.extend(side_writes(child));
    }
    ws
}

fn is_sense(path: &Path) -> bool {
    matches!(path.segments().first(), Some(Segment::Key(k)) if k == "sense")
}

/// Is `prefix` a path-prefix of `full` (prefix-aware, §6.2)?
fn prefix_of(prefix: &Path, full: &Path) -> bool {
    let (p, f) = (prefix.segments(), full.segments());
    p.len() <= f.len() && p == &f[..p.len()]
}

/// The write paths introduced directly by this node (not its children).
fn write_paths(node: &Node) -> Vec<Path> {
    match &node.kind {
        Kind::Call { into, .. }
        | Kind::Map { into, .. }
        | Kind::Filter { into, .. }
        | Kind::Try { err_into: into, .. } => vec![into.clone()],
        Kind::Tee { side_root, .. } => vec![side_root.clone()],
        Kind::Park { into: Some(p), .. } => vec![p.clone()],
        Kind::Let { bindings, .. } => bindings
            .iter()
            .filter_map(|(k, _)| Path::parse(k).ok())
            .collect(),
        _ => vec![],
    }
}

/// Immediate child instruction nodes (for recursive walks).
fn children(node: &Node) -> Vec<&Node> {
    match &node.kind {
        Kind::Seq(steps) | Kind::Fallback(steps) => steps.iter().collect(),
        Kind::Let { body, .. }
        | Kind::Loop { body, .. }
        | Kind::Guard { body, .. }
        | Kind::Budget { body, .. }
        | Kind::Timeout { body, .. }
        | Kind::Once { body, .. }
        | Kind::Map { body, .. } => vec![body],
        Kind::Branch { then, els, .. } => {
            let mut v = vec![then.as_ref()];
            if let Some(e) = els {
                v.push(e);
            }
            v
        }
        Kind::Try { body, catch, finally, .. } => {
            let mut v = vec![body.as_ref()];
            v.extend(catch.iter().map(|(_, n)| n));
            if let Some(f) = finally {
                v.push(f);
            }
            v
        }
        Kind::Tee { body, side, .. } => vec![body, side],
        _ => vec![],
    }
}
