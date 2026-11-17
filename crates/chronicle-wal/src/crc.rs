//! CRC32C (Castagnoli) checksums with the domain separation of
//! ARCHITECTURE.md section 9.3. The polynomial is 0x1EDC6F41 in
//! reflected form 0x82F63B78. A frame checksum covers the WAL domain
//! prefix, one zero byte, the first 28 header bytes, and the payload.
//! A snapshot checksum covers the snapshot domain prefix, one zero
//! byte, and every byte before the trailer. The two domains never
//! validate each other's bytes (INV-WAL-3).
//!
//! The checksum is computed here, table driven, rather than by an
//! external crate: the format pins the polynomial and the golden
//! check value, and a local implementation keeps the workspace free
//! of a dependency whose common form (CRC-32 IEEE) is a different
//! polynomial. Section 9.3's crate mention is corrected in the
//! Milestone 3 docs sync.
//!
//! The update loop is slicing-by-8 (programme 2, lane S3): eight
//! tables derived from the one byte table let each step consume
//! eight input bytes, and the tail below eight bytes runs the
//! original byte-at-a-time step. The values produced are identical
//! to the byte-at-a-time form on every input, which the test suite
//! proves against a bit-at-a-time reference written straight from
//! the polynomial.

/// Domain prefix for write-ahead log frames.
pub const WAL_DOMAIN: &[u8] = b"chronicle-wal/v1";
/// Domain prefix for snapshots.
pub const SNAPSHOT_DOMAIN: &[u8] = b"chronicle-snapshot/v1";

/// Reflected Castagnoli polynomial.
const POLYNOMIAL: u32 = 0x82F6_3B78;

/// Slicing-by-8 lookup tables, built at compile time. Table 0 is
/// the byte-at-a-time table; table k is table k-1 advanced by one
/// byte step, so a byte with k bytes still behind it in the step
/// is looked up in table k.
const TABLES: [[u32; 256]; 8] = build_tables();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

const fn build_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    tables[0] = build_table();
    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            let advanced = tables[k - 1][i];
            tables[k][i] = (advanced >> 8) ^ tables[0][(advanced & 0xFF) as usize];
            i += 1;
        }
        k += 1;
    }
    tables
}

/// Streaming CRC32C state.
#[derive(Clone, Debug)]
pub struct Crc32c {
    state: u32,
}

impl Crc32c {
    /// A fresh checksum state.
    #[must_use]
    pub const fn new() -> Self {
        Self { state: u32::MAX }
    }

    /// Feed one slice of input.
    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.state;
        let (chunks, remainder) = data.as_chunks::<8>();
        for chunk in chunks {
            let low = crc ^ u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let high = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            crc = TABLES[7][(low & 0xFF) as usize]
                ^ TABLES[6][((low >> 8) & 0xFF) as usize]
                ^ TABLES[5][((low >> 16) & 0xFF) as usize]
                ^ TABLES[4][((low >> 24) & 0xFF) as usize]
                ^ TABLES[3][(high & 0xFF) as usize]
                ^ TABLES[2][((high >> 8) & 0xFF) as usize]
                ^ TABLES[1][((high >> 16) & 0xFF) as usize]
                ^ TABLES[0][((high >> 24) & 0xFF) as usize];
        }
        for &byte in remainder {
            let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
            crc = TABLES[0][index] ^ (crc >> 8);
        }
        self.state = crc;
    }

    /// The checksum of everything fed so far.
    #[must_use]
    pub const fn finalize(&self) -> u32 {
        !self.state
    }
}

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC32C of a single slice, with no domain prefix.
#[must_use]
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(data);
    crc.finalize()
}

/// The checksum stored in a frame header: WAL domain prefix, one zero
/// byte, header bytes 0..28, then the payload (section 9.3).
#[must_use]
pub fn frame_crc(header_prefix: &[u8], payload: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(WAL_DOMAIN);
    crc.update(&[0]);
    crc.update(header_prefix);
    crc.update(payload);
    crc.finalize()
}

/// The checksum stored in a snapshot trailer: snapshot domain prefix,
/// one zero byte, then every preceding byte of the file (section
/// 10.2).
#[must_use]
pub fn snapshot_crc(preceding: &[u8]) -> u32 {
    let mut crc = Crc32c::new();
    crc.update(SNAPSHOT_DOMAIN);
    crc.update(&[0]);
    crc.update(preceding);
    crc.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-at-a-time CRC32C written straight from the polynomial,
    /// with no tables at all. This is the ground truth the slicing
    /// implementation is proven against.
    fn reference_crc32c(data: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ POLYNOMIAL
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// `SplitMix64`, the same deterministic generator the mutation
    /// harness uses, so the test data needs no external source.
    fn split_mix64(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[test]
    fn crc32c_check_value() {
        // The standard CRC-32C check value for the ASCII string
        // "123456789", published with the polynomial definition.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn crc32c_streaming_matches_one_shot() {
        let data: Vec<u8> = (0u8..=255).collect();
        let mut streaming = Crc32c::new();
        for chunk in data.chunks(7) {
            streaming.update(chunk);
        }
        assert_eq!(streaming.finalize(), crc32c(&data));
    }

    #[test]
    fn crc32c_slicing_matches_bit_at_a_time_reference() {
        // Every length from empty through several slicing steps
        // and every tail remainder, over deterministic data, in
        // one shot and in streaming splits that cross the 8-byte
        // step boundary at different phases.
        let mut state = 0xC0FF_EE15_600D_5EED_u64;
        let mut data = vec![0u8; 517];
        for byte in &mut data {
            *byte = split_mix64(&mut state).to_le_bytes()[0];
        }
        for len in 0..=data.len() {
            let slice = &data[..len];
            let expected = reference_crc32c(slice);
            assert_eq!(crc32c(slice), expected, "one shot at length {len}");
            for split in [1_usize, 3, 7, 8, 9, 64] {
                let mut streaming = Crc32c::new();
                for chunk in slice.chunks(split) {
                    streaming.update(chunk);
                }
                assert_eq!(
                    streaming.finalize(),
                    expected,
                    "streaming split {split} at length {len}"
                );
            }
        }
    }

    #[test]
    fn crc_domains_are_separate() {
        // INV-WAL-3: bytes checksummed under the WAL domain do not
        // validate under the snapshot domain, and the reverse.
        let bytes = b"some header and payload bytes";
        assert_ne!(frame_crc(bytes, b""), snapshot_crc(bytes));
    }
}
