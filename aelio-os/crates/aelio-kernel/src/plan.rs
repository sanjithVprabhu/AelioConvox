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
//! Registered flow call graphs are checked for acyclicity by the deployment-grade pass.

use crate::error::{ErrV1, ReasonCode};
use crate::instr::{Kind, Node};
use crate::registry::{Origin, Registry};
use crate::waves;
use aelio_sol::{Path, Segment};
use std::collections::HashSet;

/// Run all P0 static checks. Returns the first violation as a `Shape`/`Policy` error.
pub fn plan(root: &Node) -> Result<(), ErrV1> {
    let mut nids = HashSet::new();
    check_unique_nids(root, &mut nids)?;
    check_writes(root)?;
    check_park_positions(root, false, false)?;
    check_tee_dataflow(root)?;
    check_map_imports(root)?;
    check_const_limits(root)?;
    Ok(())
}

/// Deployment-grade planning adds registry/policy/tenant checks that cannot be performed by the
/// syntax-only [`plan`] pass.
pub fn plan_with_registry(root: &Node, registry: &Registry, tenant: &str) -> Result<(), ErrV1> {
    plan(root)?;
    if tenant.trim().is_empty() {
        return Err(ErrV1::new(
            ReasonCode::Policy,
            &root.nid,
            "deployment tenant must be non-empty",
        ));
    }
    registry
        .validate_call_graph(tenant)
        .map_err(|error| ErrV1::new(ReasonCode::Shape, &root.nid, error))?;
    check_registered_calls(root, registry, tenant)
}

fn check_registered_calls(node: &Node, registry: &Registry, tenant: &str) -> Result<(), ErrV1> {
    if let Kind::Call { id, .. } = &node.kind {
        let declaration = registry.declaration(id).ok_or_else(|| {
            ErrV1::new(
                ReasonCode::Policy,
                &node.nid,
                format!("Call target `{id}` lacks a production registry declaration"),
            )
        })?;
        declaration
            .validate()
            .map_err(|error| ErrV1::new(ReasonCode::Policy, &node.nid, error))?;
        if declaration.origin == Origin::Tenant && declaration.tenant != tenant {
            return Err(ErrV1::new(
                ReasonCode::Policy,
                &node.nid,
                format!("Call target `{id}` belongs to a different tenant (§17.2)"),
            ));
        }
    }
    for child in children(node) {
        check_registered_calls(child, registry, tenant)?;
    }
    Ok(())
}

fn check_unique_nids(node: &Node, seen: &mut HashSet<String>) -> Result<(), ErrV1> {
    if node.nid.is_empty() {
        return Err(ErrV1::new(
            ReasonCode::Shape,
            &node.nid,
            "nid must not be empty (§8.1)",
        ));
    }
    if !seen.insert(node.nid.clone()) {
        return Err(ErrV1::new(
            ReasonCode::Shape,
            &node.nid,
            "duplicate nid in one plan (§8.1)",
        ));
    }
    for child in children(node) {
        check_unique_nids(child, seen)?;
    }
    Ok(())
}

/// §8.2 / §4.4: a `Const` literal is charged against the structural limits **at plan time** — an
/// oversized literal is a broken flow, rejected at push, not discovered at runtime.
fn check_const_limits(node: &Node) -> Result<(), ErrV1> {
    if let Kind::Const(v) = &node.kind {
        aelio_sol::Limits::default().check(v).map_err(|e| {
            ErrV1::new(
                ReasonCode::BudgetSize,
                &node.nid,
                format!("§4.4 Const literal exceeds limits: {e}"),
            )
        })?;
    }
    for child in children(node) {
        check_const_limits(child)?;
    }
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

/// Park legality (§8.4 matrix). A suspended `Once` would leave an effect intent operationally
/// indistinguishable from an unknown-outcome crash, so v1 rejects it until a distinct durable
/// parked-once state exists.
fn check_park_positions(node: &Node, in_tee_side: bool, in_once: bool) -> Result<(), ErrV1> {
    if in_tee_side {
        if let Kind::Park { .. } = node.kind {
            return Err(ErrV1::new(
                ReasonCode::Shape,
                &node.nid,
                "Park is forbidden inside Tee.side (§8.4 matrix)",
            ));
        }
    }
    if in_once && matches!(node.kind, Kind::Park { .. }) {
        return Err(ErrV1::new(
            ReasonCode::Shape,
            &node.nid,
            "Park is forbidden inside Once until parked-once has a distinct durable state (§8.4)",
        ));
    }
    match &node.kind {
        Kind::Tee {
            body,
            side,
            side_root,
        } => {
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
            check_park_positions(body, in_tee_side, in_once)?;
            check_park_positions(side, true, in_once)?;
            Ok(())
        }
        Kind::Once { body, .. } => check_park_positions(body, in_tee_side, true),
        _ => {
            for child in children(node) {
                check_park_positions(child, in_tee_side, in_once)?;
            }
            Ok(())
        }
    }
}

/// A Tee side branch may not write anything read by a subsequent Seq step (§8.2). This is what
/// makes its failure/non-observation semantically incapable of contaminating the main dataflow.
fn check_tee_dataflow(node: &Node) -> Result<(), ErrV1> {
    if let Kind::Seq(steps) = &node.kind {
        for (index, step) in steps.iter().enumerate() {
            if let Kind::Tee { side, .. } = &step.kind {
                let writes = side_writes(side);
                let later_reads: Vec<Path> = steps[index + 1..]
                    .iter()
                    .flat_map(|later| waves::rw_set(later).reads)
                    .collect();
                if paths_intersect(&writes, &later_reads) {
                    return Err(ErrV1::new(
                        ReasonCode::Shape,
                        &step.nid,
                        "Tee.side write intersects a subsequent read (§8.2)",
                    ));
                }
            }
        }
    }
    for child in children(node) {
        check_tee_dataflow(child)?;
    }
    Ok(())
}

/// Map imports are explicit read-only projections. A child plan that can write an imported alias
/// is rejected before execution (§6.3).
fn check_map_imports(node: &Node) -> Result<(), ErrV1> {
    if let Kind::Map { imports, body, .. } = &node.kind {
        let writes = waves::rw_set(body).writes;
        for (alias, _) in imports {
            let alias_path = Path::parse(alias).map_err(|e| {
                ErrV1::new(
                    ReasonCode::Shape,
                    &node.nid,
                    format!("Map import alias must be a literal child path: {e}"),
                )
            })?;
            if paths_intersect(std::slice::from_ref(&alias_path), &writes) {
                return Err(ErrV1::new(
                    ReasonCode::Policy,
                    &node.nid,
                    format!("Map body writes read-only import `{alias}` (§6.3)"),
                ));
            }
        }
    }
    for child in children(node) {
        check_map_imports(child)?;
    }
    Ok(())
}

fn paths_intersect(a: &[Path], b: &[Path]) -> bool {
    a.iter().any(|left| {
        b.iter()
            .any(|right| prefix_of(left, right) || prefix_of(right, left))
    })
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
        Kind::Try {
            body,
            catch,
            finally,
            ..
        } => {
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
