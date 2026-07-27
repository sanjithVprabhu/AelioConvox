//! Wave-legality analysis (§6.2). v0 executes sequentially, but R/W sets are recorded from day one
//! and the wave-legality rule is normative *now*, so parallelism later is an **executor** change,
//! never a spec change. Two ops may share a wave iff:
//!
//! ```text
//! W₁ ∩ (R₂ ∪ W₂) = ∅   and   W₂ ∩ R₁ = ∅
//! ```
//!
//! with **prefix-aware** intersection: a write to `P` covers all descendants of `P` (§6.2). This
//! module derives each node-subtree's read and write sets from literal paths and decides legality.

use crate::instr::{Expr, Kind, Node};
use aelio_sol::{Path, Segment};

/// The read and write path sets of a node subtree.
#[derive(Debug, Default, Clone)]
pub struct RwSet {
    pub reads: Vec<Path>,
    pub writes: Vec<Path>,
}

/// Compute the (prefix-uncollapsed) read/write sets of a node subtree.
pub fn rw_set(node: &Node) -> RwSet {
    let mut set = RwSet::default();
    collect(node, &mut set);
    set
}

/// §6.2 wave-legality: can these two ops (subtrees) share a wave?
pub fn can_share_wave(a: &Node, b: &Node) -> bool {
    let (ra, rb) = (rw_set(a), rw_set(b));
    // W₁ ∩ (R₂ ∪ W₂) = ∅
    if intersects(&ra.writes, &rb.reads) || intersects(&ra.writes, &rb.writes) {
        return false;
    }
    // W₂ ∩ R₁ = ∅
    if intersects(&rb.writes, &ra.reads) {
        return false;
    }
    true
}

fn collect(node: &Node, set: &mut RwSet) {
    // Writes introduced directly by this op.
    match &node.kind {
        Kind::Call { into, args, .. } => {
            set.writes.push(into.clone());
            for (_, e) in args {
                expr_reads(e, set);
            }
        }
        Kind::Map {
            into,
            over,
            imports,
            ..
        } => {
            set.writes.push(into.clone());
            set.reads.push(over.clone());
            for (_, p) in imports {
                set.reads.push(p.clone());
            }
        }
        Kind::Filter {
            into, over, pred, ..
        } => {
            set.writes.push(into.clone());
            set.reads.push(over.clone());
            expr_reads(pred, set);
        }
        Kind::Try { err_into, .. } => set.writes.push(err_into.clone()),
        Kind::Tee { side_root, .. } => set.writes.push(side_root.clone()),
        Kind::Park { into: Some(p), .. } => set.writes.push(p.clone()),
        Kind::Let { bindings, .. } => {
            for (k, e) in bindings {
                if let Ok(p) = Path::parse(k) {
                    set.writes.push(p);
                }
                expr_reads(e, set);
            }
        }
        Kind::Branch { pred, .. } => expr_reads(pred, set),
        Kind::Loop { while_, .. } => expr_reads(while_, set),
        Kind::Guard { invariant, .. } => expr_reads(invariant, set),
        Kind::Once {
            idem_key: Some(exprs),
            ..
        } => {
            for e in exprs {
                expr_reads(e, set);
            }
        }
        _ => {}
    }
    for child in child_nodes(node) {
        collect(child, set);
    }
}

fn expr_reads(expr: &Expr, set: &mut RwSet) {
    match expr {
        Expr::Lit(_) => {}
        Expr::Pull(p) => set.reads.push(p.clone()),
        Expr::Fn { args, .. } => {
            for a in args {
                expr_reads(a, set);
            }
        }
    }
}

fn child_nodes(node: &Node) -> Vec<&Node> {
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

/// Prefix-aware intersection between two path sets (§6.2): any pair where one path is a prefix of
/// the other (a write to `P` covers `P.*`).
fn intersects(a: &[Path], b: &[Path]) -> bool {
    a.iter().any(|pa| b.iter().any(|pb| prefix_overlap(pa, pb)))
}

fn prefix_overlap(a: &Path, b: &Path) -> bool {
    let (sa, sb) = (a.segments(), b.segments());
    let n = sa.len().min(sb.len());
    seg_eq(&sa[..n], &sb[..n])
}

fn seg_eq(a: &[Segment], b: &[Segment]) -> bool {
    a == b
}
