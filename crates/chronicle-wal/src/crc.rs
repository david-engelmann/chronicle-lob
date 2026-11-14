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

/// Domain prefix for write-ahead log frames.
pub(crate) const WAL_DOMAIN: &[u8] = b"chronicle-wal/v1";
/// Domain prefix for snapshots.
pub(crate) const SNAPSHOT_DOMAIN: &[u8] = b"chronicle-snapshot/v1";

/// Reflected Castagnoli polynomial.
const POLYNOMIAL: u32 = 0x82F6_3B78;

/// Byte-at-a-time lookup table, built at compile time.
const TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
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
        for &byte in data {
            let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
            crc = TABLE[index] ^ (crc >> 8);
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
    fn crc_domains_are_separate() {
        // INV-WAL-3: bytes checksummed under the WAL domain do not
        // validate under the snapshot domain, and the reverse.
        let bytes = b"some header and payload bytes";
        assert_ne!(frame_crc(bytes, b""), snapshot_crc(bytes));
    }
}
