//! 8-bit scalar quantization for HNSW traversal.
//!
//! Per-dimension min/scale learned from the dataset (Faiss `QT_8bit`-style). Traversal
//! uses these compact codes (4× smaller than f32, cache-friendly); exact f32 vectors are
//! kept for rerank, so quantization error is recovered. The on-disk HNSW page stores the
//! same codes; the per-dimension `min`/`scale` live once in the section header.
//!
//! Codes are `u8` (0..=255); the byte-layout calls this the "int8" path — it is 8-bit.

/// Per-dimension scalar quantizer.
#[derive(Debug, Clone)]
pub struct ScalarQuantizer {
    /// Per-dimension minimum (the dequant offset).
    pub min: Vec<f32>,
    /// Per-dimension scale: `(max - min) / 255` (1.0 for constant dimensions).
    pub scale: Vec<f32>,
}

impl ScalarQuantizer {
    pub fn dim(&self) -> usize {
        self.min.len()
    }

    /// Learn per-dimension min/scale from the dataset.
    pub fn train(vectors: &[Vec<f32>], dim: usize) -> Self {
        let mut min = vec![f32::INFINITY; dim];
        let mut max = vec![f32::NEG_INFINITY; dim];
        for v in vectors {
            for (d, &x) in v.iter().enumerate() {
                if x < min[d] {
                    min[d] = x;
                }
                if x > max[d] {
                    max[d] = x;
                }
            }
        }
        if vectors.is_empty() {
            min = vec![0.0; dim];
            max = vec![0.0; dim];
        }
        let scale = (0..dim)
            .map(|d| {
                let range = max[d] - min[d];
                if range > 0.0 {
                    range / 255.0
                } else {
                    1.0
                }
            })
            .collect();
        ScalarQuantizer { min, scale }
    }

    /// Quantize one vector to 8-bit codes.
    pub fn quantize(&self, v: &[f32]) -> Vec<u8> {
        v.iter()
            .enumerate()
            .map(|(d, &x)| {
                let q = ((x - self.min[d]) / self.scale[d]).round();
                q.clamp(0.0, 255.0) as u8
            })
            .collect()
    }

    /// Dequantize back to approximate f32 (used in tests; rerank uses true f32).
    pub fn dequantize(&self, codes: &[u8]) -> Vec<f32> {
        codes
            .iter()
            .enumerate()
            .map(|(d, &c)| self.min[d] + c as f32 * self.scale[d])
            .collect()
    }

    /// Squared-L2 distance between two code vectors in dequantized space:
    /// `sum_d (scale_d * (a_d - b_d))^2`. Monotonic with the dequantized true L2.
    /// SIMD-accelerated (AVX2) via [`crate::simd::l2_quantized_scaled`].
    pub fn l2_quantized(&self, a: &[u8], b: &[u8]) -> f32 {
        crate::simd::l2_quantized_scaled(a, b, &self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dequantize_is_close_to_original() {
        let vectors = vec![vec![0.0, 10.0], vec![5.0, -10.0], vec![2.5, 0.0]];
        let q = ScalarQuantizer::train(&vectors, 2);
        for v in &vectors {
            let codes = q.quantize(v);
            let back = q.dequantize(&codes);
            for (a, b) in v.iter().zip(&back) {
                assert!((a - b).abs() <= q.scale.iter().cloned().fold(0.0, f32::max));
            }
        }
    }

    #[test]
    fn constant_dimension_does_not_divide_by_zero() {
        let vectors = vec![vec![3.0, 1.0], vec![3.0, 2.0]];
        let q = ScalarQuantizer::train(&vectors, 2);
        assert_eq!(q.scale[0], 1.0); // constant dim → scale 1.0
        assert_eq!(q.quantize(&[3.0, 2.0])[0], 0);
    }
}
