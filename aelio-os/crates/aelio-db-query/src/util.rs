//! Small shared helpers for source implementations.

/// Squared-L2 distance (SIMD-accelerated via aelio-db-index).
pub(crate) fn l2_sq(a: &[f32], b: &[f32]) -> f32 {
    aelio_db_index::l2_squared(a, b)
}

/// Sort `(row_id, score)` and keep the top `k`. `ascending` for distances (smaller first),
/// otherwise descending (larger relevance first). Ties broken by row id.
pub(crate) fn top_k(mut scored: Vec<(u64, f32)>, k: usize, ascending: bool) -> Vec<(u64, f32)> {
    scored.sort_by(|a, b| {
        if ascending {
            a.1.total_cmp(&b.1).then(a.0.cmp(&b.0))
        } else {
            b.1.total_cmp(&a.1).then(a.0.cmp(&b.0))
        }
    });
    scored.truncate(k);
    scored
}
