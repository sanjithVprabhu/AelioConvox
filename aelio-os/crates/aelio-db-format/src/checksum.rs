//! CRC32C (Castagnoli) — the checksum used at all three layers (page/section/footer).
//!
//! Software, table-driven, dependency-free. The reflected Castagnoli polynomial is
//! `0x82F63B78`. A hardware-accelerated implementation (the `crc32c` crate, using the
//! SSE4.2 `crc32` instruction) can replace this later without changing on-disk bytes.

/// Reflected Castagnoli polynomial.
const POLY: u32 = 0x82F6_3B78;

/// Build the 256-entry lookup table at compile time.
const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = build_table();

/// Compute the CRC32C of `data`.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::crc32c;

    #[test]
    fn known_vectors() {
        // Standard CRC32C check value for the ASCII string "123456789".
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        // CRC32C of the empty input is 0.
        assert_eq!(crc32c(b""), 0x0000_0000);
    }

    #[test]
    fn single_bit_flip_changes_crc() {
        let a = crc32c(b"the quick brown fox");
        let b = crc32c(b"the quick brown fox.");
        assert_ne!(a, b);
    }
}
