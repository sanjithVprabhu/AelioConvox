//! A small bloom filter over target row ids, used to prune cross-file reverse traversal:
//! reverse-lookup of T checks each file's bloom first and only consults files that say
//! "maybe". No false negatives; ~1% false positives at the default sizing.

/// SplitMix64 finalizer — a good 64-bit avalanche hash.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[derive(Debug, Clone)]
pub struct Bloom {
    bits: Vec<u8>,
    num_bits: u32,
    num_hashes: u32,
}

impl Bloom {
    /// Build a filter over `items`, sized for `expected` distinct entries (~10 bits each).
    pub fn build(items: impl Iterator<Item = u64>, expected: usize) -> Self {
        let num_bits = ((expected.max(1) as u32).saturating_mul(10))
            .next_multiple_of(8)
            .max(64);
        let num_hashes = 7;
        let mut bits = vec![0u8; (num_bits / 8) as usize];
        for it in items {
            for pos in Self::positions(it, num_bits, num_hashes) {
                bits[(pos / 8) as usize] |= 1 << (pos % 8);
            }
        }
        Bloom {
            bits,
            num_bits,
            num_hashes,
        }
    }

    pub fn from_parts(bits: Vec<u8>, num_bits: u32, num_hashes: u32) -> Self {
        Bloom {
            bits,
            num_bits,
            num_hashes,
        }
    }

    pub fn num_bits(&self) -> u32 {
        self.num_bits
    }
    pub fn num_hashes(&self) -> u32 {
        self.num_hashes
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bits
    }

    fn positions(item: u64, num_bits: u32, num_hashes: u32) -> impl Iterator<Item = u32> {
        let h1 = mix(item);
        let h2 = mix(item ^ 0x9E37_79B9_7F4A_7C15) | 1;
        let nb = num_bits as u64;
        (0..num_hashes).map(move |i| (h1.wrapping_add((i as u64).wrapping_mul(h2)) % nb) as u32)
    }

    /// True if `item` may be present (definitely-not when false). A malformed filter
    /// (zero bits) safely reports "not present" rather than dividing by zero.
    pub fn might_contain(&self, item: u64) -> bool {
        if self.num_bits == 0 || self.num_hashes == 0 {
            return false;
        }
        Self::positions(item, self.num_bits, self.num_hashes)
            .all(|pos| (pos / 8) < self.bits.len() as u32 && self.bits[(pos / 8) as usize] & (1 << (pos % 8)) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::Bloom;

    #[test]
    fn no_false_negatives() {
        let items: Vec<u64> = (0..1000).map(|i| i * 7 + 3).collect();
        let b = Bloom::build(items.iter().copied(), items.len());
        for &it in &items {
            assert!(b.might_contain(it), "missing {it}");
        }
    }

    #[test]
    fn prunes_absent_items() {
        let items: Vec<u64> = (0..1000).map(|i| i * 2).collect(); // evens
        let b = Bloom::build(items.iter().copied(), items.len());
        // Odd numbers were never inserted; most should be pruned.
        let pruned = (0..1000).filter(|i| !b.might_contain(i * 2 + 1)).count();
        assert!(pruned > 900, "bloom pruned only {pruned}/1000");
    }
}
