//! SIMD-accelerated squared-L2 distance — the hottest kernel in the system (HNSW build,
//! traversal rerank). AVX2+FMA on x86_64 with runtime detection; portable scalar fallback.

/// Squared Euclidean distance between two equal-length f32 slices.
pub fn l2_squared(a: &[f32], b: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
            // SAFETY: only called after confirming the CPU supports AVX2 + FMA.
            return unsafe { l2_avx2(a, b) };
        }
    }
    l2_scalar(a, b)
}

#[inline]
fn l2_scalar(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = x - y;
            d * d
        })
        .sum()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn l2_avx2(a: &[f32], b: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let len = a.len().min(b.len());
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i + 8 <= len {
        let va = _mm256_loadu_ps(a.as_ptr().add(i));
        let vb = _mm256_loadu_ps(b.as_ptr().add(i));
        let d = _mm256_sub_ps(va, vb);
        acc = _mm256_fmadd_ps(d, d, acc); // acc += d*d
        i += 8;
    }
    let mut tmp = [0f32; 8];
    _mm256_storeu_ps(tmp.as_mut_ptr(), acc);
    let mut s = tmp[0] + tmp[1] + tmp[2] + tmp[3] + tmp[4] + tmp[5] + tmp[6] + tmp[7];
    while i < len {
        let d = a[i] - b[i];
        s += d * d;
        i += 1;
    }
    s
}

/// Quantized squared-L2 between two 8-bit code vectors with per-dimension scale:
/// `sum_d (scale_d * (a_d - b_d))^2`. The hot kernel during HNSW traversal.
pub fn l2_quantized_scaled(a: &[u8], b: &[u8], scale: &[f32]) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") {
            // SAFETY: guarded by runtime AVX2 + FMA detection.
            return unsafe { l2q_avx2(a, b, scale) };
        }
    }
    l2q_scalar(a, b, scale)
}

#[inline]
fn l2q_scalar(a: &[u8], b: &[u8], scale: &[f32]) -> f32 {
    let mut acc = 0f32;
    for d in 0..a.len().min(b.len()).min(scale.len()) {
        let diff = (a[d] as f32 - b[d] as f32) * scale[d];
        acc += diff * diff;
    }
    acc
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn l2q_avx2(a: &[u8], b: &[u8], scale: &[f32]) -> f32 {
    use std::arch::x86_64::*;
    let len = a.len().min(b.len()).min(scale.len());
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i + 8 <= len {
        // load 8 bytes, zero-extend to i32, convert to f32
        let ai = _mm256_cvtepi32_ps(_mm256_cvtepu8_epi32(_mm_loadl_epi64(
            a.as_ptr().add(i) as *const _
        )));
        let bi = _mm256_cvtepi32_ps(_mm256_cvtepu8_epi32(_mm_loadl_epi64(
            b.as_ptr().add(i) as *const _
        )));
        let sc = _mm256_loadu_ps(scale.as_ptr().add(i));
        let d = _mm256_mul_ps(_mm256_sub_ps(ai, bi), sc);
        acc = _mm256_fmadd_ps(d, d, acc);
        i += 8;
    }
    let mut tmp = [0f32; 8];
    _mm256_storeu_ps(tmp.as_mut_ptr(), acc);
    let mut s = tmp[0] + tmp[1] + tmp[2] + tmp[3] + tmp[4] + tmp[5] + tmp[6] + tmp[7];
    while i < len {
        let diff = (a[i] as f32 - b[i] as f32) * scale[i];
        s += diff * diff;
        i += 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simd_matches_scalar() {
        for dim in [1usize, 7, 8, 16, 31, 32, 100, 257] {
            let a: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.5 - 3.0).collect();
            let b: Vec<f32> = (0..dim).map(|i| (i as f32) * -0.3 + 1.0).collect();
            let s = l2_scalar(&a, &b);
            let v = l2_squared(&a, &b);
            assert!(
                (s - v).abs() <= 1e-3 * (1.0 + s.abs()),
                "dim {dim}: {s} vs {v}"
            );
        }
    }

    #[test]
    fn quantized_simd_matches_scalar() {
        for dim in [1usize, 8, 15, 32, 100] {
            let a: Vec<u8> = (0..dim).map(|i| (i * 7 % 256) as u8).collect();
            let b: Vec<u8> = (0..dim).map(|i| ((i * 3 + 11) % 256) as u8).collect();
            let scale: Vec<f32> = (0..dim).map(|i| 0.01 + i as f32 * 0.001).collect();
            let s = l2q_scalar(&a, &b, &scale);
            let v = l2_quantized_scaled(&a, &b, &scale);
            assert!(
                (s - v).abs() <= 1e-3 * (1.0 + s.abs()),
                "dim {dim}: {s} vs {v}"
            );
        }
    }
}
