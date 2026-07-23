//! The unified hybrid query executor — "one query plan".
//!
//! A [`Query`] combines first-class operators (vector search, text match, graph path,
//! scalar filters). [`execute`] fans each anchor out to **every** [`Source`] (memtable +
//! files), merges candidates by global `row_id`, applies graph + scalar filters, and fuses
//! the per-modality rankings with **Reciprocal Rank Fusion** — so recent and persisted
//! data across all four modalities are planned and ranked as a single query.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use ll_cost::{choose, Strategy, VecCostParams};
use ll_engine::Value;

use crate::source::Source;
use crate::util::l2_sq;

/// Scalar comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

/// A scalar predicate on a column.
#[derive(Debug, Clone)]
pub struct Predicate {
    pub col: u32,
    pub op: PredOp,
    pub value: Value,
}

/// A graph reachability constraint: candidates must be reachable from `seeds` along edge
/// column `col` within `max_depth` hops.
#[derive(Debug, Clone)]
pub struct GraphConstraint {
    pub col: u32,
    pub seeds: Vec<u64>,
    pub max_depth: usize,
    pub budget: GraphBudget,
    pub scope: Vec<Predicate>,
}

/// Hard limits for graph expansion. Limits include seeds and every discovered node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphBudget {
    pub max_seeds: usize,
    pub max_depth: usize,
    pub max_frontier: usize,
    pub max_visited: usize,
    pub max_elapsed: Duration,
    pub deadline: Option<Instant>,
}

impl Default for GraphBudget {
    fn default() -> Self {
        Self {
            max_seeds: 64,
            max_depth: 8,
            max_frontier: 10_000,
            max_visited: 100_000,
            max_elapsed: Duration::from_millis(250),
            deadline: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphBudgetKind {
    Seeds,
    Depth,
    Frontier,
    Visited,
    Elapsed,
    Deadline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphBudgetExceeded {
    pub kind: GraphBudgetKind,
    pub limit: u128,
    pub observed: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    GraphBudgetExceeded(GraphBudgetExceeded),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GraphBudgetExceeded(exceeded) => write!(
                f,
                "graph {:?} budget exceeded: observed {}, limit {}",
                exceeded.kind, exceeded.observed, exceeded.limit
            ),
        }
    }
}

impl std::error::Error for QueryError {}

impl GraphBudgetExceeded {
    fn query_error(kind: GraphBudgetKind, limit: usize, observed: usize) -> QueryError {
        QueryError::GraphBudgetExceeded(Self {
            kind,
            limit: limit as u128,
            observed: observed as u128,
        })
    }
}

/// A hybrid query (columns identified by id). At least one ranking anchor (vector or text)
/// or a graph constraint should be present.
#[derive(Debug, Clone)]
pub struct Query {
    pub k: usize,
    pub vector: Option<(u32, Vec<f32>)>,
    pub text: Option<(u32, String)>,
    pub filters: Vec<Predicate>,
    pub graph: Option<GraphConstraint>,
    pub rrf_k: f32,
}

impl Query {
    pub fn new(k: usize) -> Self {
        Query {
            k,
            vector: None,
            text: None,
            filters: Vec::new(),
            graph: None,
            rrf_k: 60.0,
        }
    }
    pub fn with_vector(mut self, col: u32, q: Vec<f32>) -> Self {
        self.vector = Some((col, q));
        self
    }
    pub fn with_text(mut self, col: u32, q: impl Into<String>) -> Self {
        self.text = Some((col, q.into()));
        self
    }
    pub fn filter(mut self, col: u32, op: PredOp, value: Value) -> Self {
        self.filters.push(Predicate { col, op, value });
        self
    }
    pub fn with_graph(mut self, col: u32, seeds: Vec<u64>, max_depth: usize) -> Self {
        self.graph = Some(GraphConstraint {
            col,
            seeds,
            max_depth,
            budget: GraphBudget::default(),
            scope: Vec::new(),
        });
        self
    }
    pub fn with_graph_budget(mut self, budget: GraphBudget) -> Self {
        if let Some(graph) = &mut self.graph {
            graph.budget = budget;
        }
        self
    }
    pub fn with_graph_scope(mut self, predicate: Predicate) -> Self {
        if let Some(graph) = &mut self.graph {
            graph.scope.push(predicate);
        }
        self
    }

    /// A human-readable description of the plan (the first-class operators it runs).
    pub fn explain(&self, num_sources: usize) -> String {
        let mut anchors = Vec::new();
        if let Some((c, _)) = &self.vector {
            anchors.push(format!("VectorSearch(col {c})"));
        }
        if let Some((c, _)) = &self.text {
            anchors.push(format!("TextMatch(col {c})"));
        }
        let mut s = format!(
            "HybridRank[RRF k={}] over [{}]",
            self.rrf_k,
            anchors.join(", ")
        );
        if !self.filters.is_empty() {
            s.push_str(&format!(" filter[{}]", self.filters.len()));
        }
        if let Some(g) = &self.graph {
            s.push_str(&format!(
                " PathReachable(col {}, depth {})",
                g.col, g.max_depth
            ));
        }
        s.push_str(&format!(" across {num_sources} sources"));
        s
    }
}

/// Execute the query across all `sources` at `snapshot`, returning top-k `(row_id, score)`.
pub fn execute(sources: &[&dyn Source], q: &Query, snapshot: u64) -> Vec<(u64, f32)> {
    execute_checked(sources, q, snapshot).unwrap_or_default()
}

/// Execute with typed graph-budget failure instead of collapsing a bounded traversal to no hits.
pub fn execute_checked(
    sources: &[&dyn Source],
    q: &Query,
    snapshot: u64,
) -> Result<Vec<(u64, f32)>, QueryError> {
    let fetch = q.k.max(1) * 4;

    // Per-modality ranked candidate lists (best first), merged across sources by row_id.
    let mut ranked_lists: Vec<Vec<u64>> = Vec::new();

    if let Some((col, query)) = &q.vector {
        // Cost model picks the strategy and (for post-filter) the search breadth `ef`.
        let plan = choose_vector_strategy(sources, *col, &q.filters, q.k, fetch, snapshot);
        let ranked = match plan.strategy {
            Strategy::PostFilter => {
                // Over-fetch ~k/selectivity so enough matches survive the filter (a fixed
                // k*4 under-fetches at moderate selectivity → recall loss).
                let pf = if q.filters.is_empty() {
                    fetch
                } else {
                    post_fetch(q.k, plan.sel, plan.total)
                };
                postfilter_vector(sources, *col, query, pf, plan.ef, snapshot)
            }
            // PreFilter → exact distance over the matching rows. (`choose` only ever returns
            // pre/post-filter; any other variant falls through to this exact, recall-safe path.)
            _ => prefilter_vector(
                sources,
                *col,
                query,
                plan.matching.as_deref().unwrap_or(&[]),
                fetch,
                snapshot,
            ),
        };
        ranked_lists.push(ranked);
    }

    if let Some((col, query)) = &q.text {
        let mut best: HashMap<u64, f32> = HashMap::new();
        for (i, s) in sources.iter().enumerate() {
            for (id, score) in s.text_search(*col, query, fetch, snapshot) {
                // Only count a hit from the source that holds the row's newest version, so a
                // stale document in an older segment can't contribute after an update/delete.
                if resolve(sources, id, snapshot) != Some((i, false)) {
                    continue;
                }
                best.entry(id)
                    .and_modify(|x| {
                        if score > *x {
                            *x = score;
                        }
                    })
                    .or_insert(score);
            }
        }
        let mut v: Vec<(u64, f32)> = best.into_iter().collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0))); // descending relevance
        ranked_lists.push(v.into_iter().map(|(id, _)| id).collect());
    }

    // Graph reachability set (if constrained).
    let reach = q
        .graph
        .as_ref()
        .map(|g| reachable(sources, g, snapshot))
        .transpose()?;

    // Candidate universe: union of ranked lists; or the reachable set for a graph-only query.
    let mut universe: BTreeSet<u64> = BTreeSet::new();
    for l in &ranked_lists {
        universe.extend(l.iter().copied());
    }
    if ranked_lists.is_empty() {
        if let Some(r) = &reach {
            universe.extend(r.iter().copied());
        } else if !q.filters.is_empty() {
            // Scalar-only scan (e.g. KV lookup by entry_key) — no vector/text anchor.
            universe.extend(matching_rows(sources, &q.filters, snapshot));
        }
    }

    // RRF fusion across the ranked lists. `rrf_k` is clamped non-negative so the denominator
    // stays ≥ 1 (a negative override can never produce a NaN/∞ score that corrupts the sort).
    let rrf_k = q.rrf_k.max(0.0);
    let mut fused: HashMap<u64, f32> = HashMap::new();
    for l in &ranked_lists {
        for (rank, id) in l.iter().enumerate() {
            *fused.entry(*id).or_insert(0.0) += 1.0 / (rrf_k + (rank as f32 + 1.0));
        }
    }

    // Resolve each candidate to its newest version across sources, then filter (version +
    // graph + scalar) and collect. A row whose newest visible version is a tombstone — or
    // which is superseded by a newer version in another source — is dropped here, and its
    // scalar filters are evaluated against the authoritative source only.
    let mut out: Vec<(u64, f32)> = Vec::new();
    for id in universe {
        let Some((src, deleted)) = resolve(sources, id, snapshot) else {
            continue;
        };
        if deleted {
            continue;
        }
        if let Some(r) = &reach {
            if !r.contains(&id) {
                continue;
            }
        }
        if !q.filters.iter().all(|p| {
            sources[src]
                .scalar(id, p.col, snapshot)
                .map(|v| passes(&v, p.op, &p.value))
                .unwrap_or(false)
        }) {
            continue;
        }
        let score = fused.get(&id).copied().unwrap_or(1.0);
        out.push((id, score));
    }
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(q.k);
    Ok(out)
}

/// Recall target the planner holds the chosen strategy to.
const RECALL_TARGET: f64 = 0.9;
/// Safety discount on the measured recall curve. The probe uses dataset points as queries
/// (on-manifold, so easier than real out-of-sample queries) and is unfiltered, both of which
/// make it optimistic; subtracting this margin before testing the target keeps the *actual*
/// query recall at or above target rather than leaking just under it.
const RECALL_MARGIN: f64 = 0.05;
/// Search breadth used when no ANN recall curve is available (exact sources only).
const DEFAULT_EF: usize = 640;

/// The chosen filtered-vector plan: which strategy, the search breadth `ef` (post-filter),
/// the matching rows (pre-filter only), and the estimated selectivity/total.
struct VectorPlan {
    strategy: Strategy,
    ef: usize,
    ann_recall: f64,
    matching: Option<Vec<u64>>,
    sel: f64,
    total: usize,
}

/// The smallest `ef` on the column's combined recall curve (per-`ef` minimum across the
/// sources serving it) whose recall meets `RECALL_TARGET`, with that recall. Falls back to the
/// largest-`ef` point (best effort) when none meets the target, or `(DEFAULT_EF, 1.0)` when no
/// source has a curve (all exact). "Search harder before giving up; give up to exact only when
/// even the widest search can't deliver."
fn pick_ef(sources: &[&dyn Source], col: u32) -> Option<(usize, f64)> {
    let curves: Vec<Vec<(usize, f64)>> = sources
        .iter()
        .map(|s| s.ann_recall_curve(col))
        .filter(|c| !c.is_empty())
        .collect();
    let first = curves.first()?; // None => all sources exact (e.g. memtable-only)
                                 // Curves share the ef ladder; combine by per-position minimum recall, then discount by the
                                 // safety margin (the probe is optimistic). The returned recall is this effective value.
    let combined: Vec<(usize, f64)> = first
        .iter()
        .enumerate()
        .map(|(i, &(ef, _))| {
            let r = curves
                .iter()
                .map(|c| c.get(i).map(|&(_, r)| r).unwrap_or(1.0))
                .fold(1.0f64, f64::min);
            (ef, (r - RECALL_MARGIN).clamp(0.0, 1.0))
        })
        .collect();
    combined
        .iter()
        .find(|&&(_, r)| r >= RECALL_TARGET)
        .copied()
        .or_else(|| combined.last().copied())
}

/// Choose the filtered-vector strategy via the cost model. With no scalar predicates, plain
/// post-filter (ANN) at the adaptively-chosen `ef`. Otherwise estimate selectivity from a
/// bounded sample and let `ll-cost` weigh post-filter (at the smallest `ef` meeting the recall
/// target) against exact pre-filter, falling back to pre-filter when the ANN can't meet the
/// target or post-filter is simply pricier.
fn choose_vector_strategy(
    sources: &[&dyn Source],
    col: u32,
    filters: &[Predicate],
    k: usize,
    fetch: usize,
    snapshot: u64,
) -> VectorPlan {
    // Smallest ef meeting the recall target (or best effort), per the measured curve. `None`
    // means all sources are exact (memtable-only): recall 1.0, and the ANN cost stays the
    // original `fetch`-based estimate rather than scaling with a (meaningless) chosen ef.
    let (ef, ann_recall, cost_ef) = match pick_ef(sources, col) {
        Some((ef, r)) => (ef, r, ef),
        None => (DEFAULT_EF, 1.0, fetch),
    };

    if filters.is_empty() {
        // Pure ANN top-k: no pre-filter alternative exists, search at the chosen ef.
        return VectorPlan {
            strategy: Strategy::PostFilter,
            ef,
            ann_recall,
            matching: None,
            sel: 1.0,
            total: 0,
        };
    }
    let (sel, total) = estimate_selectivity(sources, filters, snapshot);
    // Post-filter's cost reflects the chosen ef (wider search = more work); its recall is the
    // measured recall at that ef. If that recall can't meet the target the model excludes
    // post-filter and falls back to exact pre-filter — so recall holds as ANN degrades with dim.
    let hnsw_base_evals = (cost_ef as f64) * (total.max(2) as f64).log2();
    let strategy = choose(
        sel,
        &VecCostParams {
            n: total,
            k,
            ef: fetch,
            hnsw_base_evals,
            base_ann_recall: ann_recall,
        },
        RECALL_TARGET,
    );
    let matching = match strategy {
        Strategy::PostFilter => None, // doesn't need the match set — skip the scan
        _ => Some(matching_rows(sources, filters, snapshot)),
    };
    VectorPlan {
        strategy,
        ef,
        ann_recall,
        matching,
        sel,
        total,
    }
}

/// Estimate predicate selectivity by sampling up to `SAMPLE` rows per source. Returns
/// `(selectivity, total_rows)`.
fn estimate_selectivity(
    sources: &[&dyn Source],
    filters: &[Predicate],
    snapshot: u64,
) -> (f64, usize) {
    const SAMPLE: usize = 2048;
    let mut seen = 0usize;
    let mut hits = 0usize;
    let mut total = 0usize;
    for s in sources {
        let rows = s.all_rows(snapshot);
        total += rows.len();
        let step = (rows.len() / SAMPLE).max(1);
        let mut i = 0;
        while i < rows.len() {
            let id = rows[i];
            if filters.iter().all(|p| {
                s.scalar(id, p.col, snapshot)
                    .map(|v| passes(&v, p.op, &p.value))
                    .unwrap_or(false)
            }) {
                hits += 1;
            }
            seen += 1;
            i += step;
        }
    }
    let sel = if seen > 0 {
        hits as f64 / seen as f64
    } else {
        1.0
    };
    (sel, total)
}

/// Candidate count for post-filter: over-fetch ~`k / selectivity` (×3 safety) so enough
/// matches survive the filter, clamped to `[k*4, total]`.
fn post_fetch(k: usize, sel: f64, total: usize) -> usize {
    let floor = (k * 8).min(total.max(1));
    if sel <= 0.0 {
        return floor;
    }
    (((k as f64 / sel) * 4.0).ceil() as usize).clamp(floor, total.max(floor))
}

/// Rows satisfying all scalar predicates, across sources (sorted, unique).
fn matching_rows(sources: &[&dyn Source], filters: &[Predicate], snapshot: u64) -> Vec<u64> {
    let mut set: BTreeSet<u64> = BTreeSet::new();
    for s in sources {
        // Fast path: a scalar index answers the predicates directly.
        if let Some(rows) = s.rows_for_predicates(filters, snapshot) {
            set.extend(rows);
            continue;
        }
        // Fallback: scan this source's rows.
        for id in s.all_rows(snapshot) {
            if filters.iter().all(|p| {
                s.scalar(id, p.col, snapshot)
                    .map(|v| passes(&v, p.op, &p.value))
                    .unwrap_or(false)
            }) {
                set.insert(id);
            }
        }
    }
    set.into_iter().collect()
}

/// Post-filter: ANN candidates from every source (searched at breadth `ef`), merged by row_id
/// (keep-min distance).
fn postfilter_vector(
    sources: &[&dyn Source],
    col: u32,
    query: &[f32],
    fetch: usize,
    ef: usize,
    snapshot: u64,
) -> Vec<u64> {
    let mut best: HashMap<u64, f32> = HashMap::new();
    for (i, s) in sources.iter().enumerate() {
        for (id, dist) in s.vector_search(col, query, fetch, ef, snapshot) {
            // Keep a hit only from the source holding the row's newest version, so a stale
            // vector in an older segment can't outrank the row's current version.
            if resolve(sources, id, snapshot) != Some((i, false)) {
                continue;
            }
            best.entry(id)
                .and_modify(|x| {
                    if dist < *x {
                        *x = dist;
                    }
                })
                .or_insert(dist);
        }
    }
    let mut v: Vec<(u64, f32)> = best.into_iter().collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(id, _)| id).collect()
}

/// Minimum matching-row count above which the exact pre-filter scan is parallelized. Below
/// it (the low-selectivity case) thread-spawn overhead would exceed the gain — and the
/// concurrent-query throughput path is the right axis there. Above it (high selectivity, or
/// the high-dimension exact fallback) the scan dominates and splitting it across cores pays.
const PREFILTER_PAR_MIN: usize = 8192;

/// Exact distance of one matching row, read from its authoritative source (skips
/// superseded/deleted rows and dimension mismatches).
fn score_row(
    sources: &[&dyn Source],
    col: u32,
    query: &[f32],
    id: u64,
    snapshot: u64,
) -> Option<(u64, f32)> {
    let (src, deleted) = resolve(sources, id, snapshot)?;
    if deleted {
        return None;
    }
    let v = sources[src].vector(col, id, snapshot)?;
    (v.len() == query.len()).then(|| (id, l2_sq(query, &v)))
}

/// Pre-filter: exact distance over the (already-filtered) matching rows — recall is exact.
/// Parallelized across cores for large match sets (`Source: Sync` makes the shared read safe);
/// serial below the threshold where thread overhead would dominate. Returns the `limit`
/// nearest (enough for fusion + top-k) via a partial select, avoiding a full sort of every
/// match — at high selectivity the match set can be the whole table.
fn prefilter_vector(
    sources: &[&dyn Source],
    col: u32,
    query: &[f32],
    matching: &[u64],
    limit: usize,
    snapshot: u64,
) -> Vec<u64> {
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut scored: Vec<(u64, f32)> = if matching.len() >= PREFILTER_PAR_MIN && nthreads > 1 {
        let chunk = matching.len().div_ceil(nthreads);
        std::thread::scope(|sc| {
            let handles: Vec<_> = matching
                .chunks(chunk)
                .map(|ch| {
                    sc.spawn(move || {
                        ch.iter()
                            .filter_map(|&id| score_row(sources, col, query, id, snapshot))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        })
    } else {
        matching
            .iter()
            .filter_map(|&id| score_row(sources, col, query, id, snapshot))
            .collect()
    };
    // Only the nearest `limit` matter downstream; partial-select instead of fully sorting.
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, |a, b| a.1.total_cmp(&b.1));
        scored.truncate(limit);
    }
    scored.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(id, _)| id).collect()
}

/// A plan description including the cost-model-chosen vector strategy, the measured ANN recall
/// at the chosen `ef`, and why that strategy was picked (for EXPLAIN).
pub fn explain_plan(sources: &[&dyn Source], q: &Query, snapshot: u64) -> String {
    let mut s = q.explain(sources.len());
    if let Some((col, _)) = &q.vector {
        let fetch = q.k.max(1) * 4;
        let plan = choose_vector_strategy(sources, *col, &q.filters, q.k, fetch, snapshot);
        let reason = match plan.strategy {
            Strategy::PostFilter if plan.ann_recall >= RECALL_TARGET => {
                format!(
                    "ANN recall {:.3} >= target {RECALL_TARGET} at ef {}",
                    plan.ann_recall, plan.ef
                )
            }
            _ if !q.filters.is_empty() && plan.ann_recall < RECALL_TARGET => {
                format!(
                    "ANN recall {:.3} < target {RECALL_TARGET} even at ef {} -> exact pre-filter",
                    plan.ann_recall, plan.ef
                )
            }
            _ => format!("est. selectivity {:.3} favors exact pre-filter", plan.sel),
        };
        s.push_str(&format!(" [vector: {:?}; {reason}]", plan.strategy));
    }
    s
}

/// Multi-source forward reachability from the seeds.
fn reachable(
    sources: &[&dyn Source],
    g: &GraphConstraint,
    snapshot: u64,
) -> Result<HashSet<u64>, QueryError> {
    if g.seeds.len() > g.budget.max_seeds {
        return Err(GraphBudgetExceeded::query_error(
            GraphBudgetKind::Seeds,
            g.budget.max_seeds,
            g.seeds.len(),
        ));
    }
    if g.max_depth > g.budget.max_depth {
        return Err(GraphBudgetExceeded::query_error(
            GraphBudgetKind::Depth,
            g.budget.max_depth,
            g.max_depth,
        ));
    }
    let started = Instant::now();
    let mut visited: HashSet<u64> = g.seeds.iter().copied().collect();
    if visited.len() > g.budget.max_visited {
        return Err(GraphBudgetExceeded::query_error(
            GraphBudgetKind::Visited,
            g.budget.max_visited,
            visited.len(),
        ));
    }
    let mut reached: HashSet<u64> = HashSet::new();
    let mut frontier: Vec<u64> = g.seeds.clone();
    for _ in 0..g.max_depth {
        check_elapsed(started, g.budget)?;
        let mut next = Vec::new();
        for &node in &frontier {
            check_elapsed(started, g.budget)?;
            let Some((source_index, deleted)) = resolve(sources, node, snapshot) else {
                continue;
            };
            if deleted || !in_scope(sources[source_index], node, &g.scope, snapshot) {
                continue;
            }
            // Only the authoritative row version contributes edges. Unioning all sources
            // resurrects removed edges after an update in a newer segment.
            for t in sources[source_index].out_neighbors(g.col, node, snapshot) {
                let Some((target_source, target_deleted)) = resolve(sources, t, snapshot) else {
                    continue;
                };
                if target_deleted || !in_scope(sources[target_source], t, &g.scope, snapshot) {
                    continue;
                }
                reached.insert(t);
                if visited.insert(t) {
                    if visited.len() > g.budget.max_visited {
                        return Err(GraphBudgetExceeded::query_error(
                            GraphBudgetKind::Visited,
                            g.budget.max_visited,
                            visited.len(),
                        ));
                    }
                    next.push(t);
                    if next.len() > g.budget.max_frontier {
                        return Err(GraphBudgetExceeded::query_error(
                            GraphBudgetKind::Frontier,
                            g.budget.max_frontier,
                            next.len(),
                        ));
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(reached)
}

fn check_elapsed(started: Instant, budget: GraphBudget) -> Result<(), QueryError> {
    if budget
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Err(QueryError::GraphBudgetExceeded(GraphBudgetExceeded {
            kind: GraphBudgetKind::Deadline,
            limit: 0,
            observed: 1,
        }));
    }
    let elapsed = started.elapsed();
    if elapsed > budget.max_elapsed {
        return Err(QueryError::GraphBudgetExceeded(GraphBudgetExceeded {
            kind: GraphBudgetKind::Elapsed,
            limit: budget.max_elapsed.as_nanos(),
            observed: elapsed.as_nanos(),
        }));
    }
    Ok(())
}

fn in_scope(source: &dyn Source, row_id: u64, scope: &[Predicate], snapshot: u64) -> bool {
    scope.iter().all(|predicate| {
        source
            .scalar(row_id, predicate.col, snapshot)
            .is_some_and(|value| passes(&value, predicate.op, &predicate.value))
    })
}

/// Resolve `id` to the source holding its newest version visible at `snapshot`, plus whether
/// that version is a tombstone. `None` if no source has a visible version. "Newest" = highest
/// version stamp (creation LSN, globally monotonic) — the global newest-version-wins rule that
/// makes an updated or deleted row suppress its stale copy in any older segment.
fn resolve(sources: &[&dyn Source], id: u64, snapshot: u64) -> Option<(usize, bool)> {
    let mut best: Option<(usize, u64, bool)> = None;
    for (i, s) in sources.iter().enumerate() {
        if let Some((stamp, deleted)) = s.version_at(id, snapshot) {
            if best.is_none_or(|(_, bstamp, _)| stamp > bstamp) {
                best = Some((i, stamp, deleted));
            }
        }
    }
    best.map(|(i, _, deleted)| (i, deleted))
}

fn passes(v: &Value, op: PredOp, target: &Value) -> bool {
    match op {
        PredOp::Eq => v == target,
        PredOp::Ne => v != target,
        _ => match value_cmp(v, target) {
            Some(ord) => match op {
                PredOp::Gt => ord == Ordering::Greater,
                PredOp::Ge => ord != Ordering::Less,
                PredOp::Lt => ord == Ordering::Less,
                PredOp::Le => ord != Ordering::Greater,
                _ => false,
            },
            None => false,
        },
    }
}

fn value_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::I64(x), Value::I64(y)) => x.partial_cmp(y),
        (Value::F64(x), Value::F64(y)) => x.partial_cmp(y),
        (Value::Utf8(x), Value::Utf8(y)) => x.partial_cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.partial_cmp(y),
        _ => None,
    }
}
