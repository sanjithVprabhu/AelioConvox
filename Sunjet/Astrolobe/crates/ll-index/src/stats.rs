//! On-disk encoding of the ANN recall-vs-`ef` curve carried in a `.vss` OptimizerStats
//! section. The planner reads it to pick the smallest `ef` that meets a recall target (or to
//! fall back to exact pre-filter when none does).

/// The `ef` values probed at flush and offered to the planner. Spans the query path's typical
/// breadth (a filtered post-filter searches at ~640 for k=10) up to "search much harder".
pub const RECALL_EF_LADDER: &[usize] = &[128, 320, 640, 1280, 2560];

/// Encode a recall-vs-`ef` curve as `[count: u16][(ef: u32, recall: f64) × count]`.
pub fn encode_recall_curve(curve: &[(usize, f64)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(2 + curve.len() * 12);
    b.extend_from_slice(&(curve.len() as u16).to_le_bytes());
    for &(ef, recall) in curve {
        b.extend_from_slice(&(ef as u32).to_le_bytes());
        b.extend_from_slice(&recall.to_le_bytes());
    }
    b
}

/// Decode a curve written by [`encode_recall_curve`]; returns whatever prefix is well-formed
/// (never panics on malformed/truncated input).
pub fn decode_recall_curve(bytes: &[u8]) -> Vec<(usize, f64)> {
    let mut out = Vec::new();
    if bytes.len() < 2 {
        return out;
    }
    let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    let mut p = 2;
    for _ in 0..count {
        let Some(efb) = bytes.get(p..p + 4) else { break };
        let Some(rb) = bytes.get(p + 4..p + 12) else { break };
        let ef = u32::from_le_bytes(efb.try_into().unwrap()) as usize;
        let recall = f64::from_le_bytes(rb.try_into().unwrap());
        out.push((ef, recall));
        p += 12;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let curve = vec![(128, 0.8123), (640, 0.951), (2560, 0.999)];
        let bytes = encode_recall_curve(&curve);
        let back = decode_recall_curve(&bytes);
        assert_eq!(curve.len(), back.len());
        for ((e0, r0), (e1, r1)) in curve.iter().zip(&back) {
            assert_eq!(e0, e1);
            assert!((r0 - r1).abs() < 1e-12);
        }
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let bytes = encode_recall_curve(&[(128, 0.5), (640, 0.9)]);
        for cut in 0..bytes.len() {
            let _ = decode_recall_curve(&bytes[..cut]); // must not panic
        }
    }
}
