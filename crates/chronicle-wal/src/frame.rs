//! Frame encoding and decoding (ARCHITECTURE.md section 9.1 and
//! 9.2). A frame is a 32 byte header plus a fixed width payload, no
//! padding. The header sequence number is authoritative; payloads
//! carry no sequence. Reserved fields are written zero and are
//! corruption when read back nonzero. Encoding is pure byte work
//! into a caller buffer, and decoding follows the decision table of
//! section 9.5 exactly (see [`parse_frame`]).

use chronicle_core::{
    Book, BookError, EventBuffer, OrderId, OrderType, ParticipantId, Side, StpPolicy,
};

use crate::crc::frame_crc;
use crate::error::{WalCorruption, WalError};

/// Frame magic bytes, ASCII `CWAL`.
pub const MAGIC: [u8; 4] = *b"CWAL";
/// The only frame format version.
pub const VERSION: u16 = 1;
/// Frame header length in bytes.
pub const HEADER_LEN: usize = 32;
/// Largest payload the format admits. Every current kind is far
/// below this; the cap exists so a corrupt length field is caught
/// before it is trusted.
pub const MAX_PAYLOAD_LEN: u32 = 4096;

/// Frame kind for an add command.
pub const KIND_ADD: u8 = 1;
/// Frame kind for a cancel command.
pub const KIND_CANCEL: u8 = 2;
/// Frame kind for a cancel by participant command.
pub const KIND_CANCEL_BY_PARTICIPANT: u8 = 3;
/// Frame kind for an amend command.
pub const KIND_AMEND: u8 = 4;

/// Payload width of an add frame.
pub const ADD_PAYLOAD_LEN: usize = 40;
/// Payload width of a cancel frame.
pub const CANCEL_PAYLOAD_LEN: usize = 16;
/// Payload width of a cancel by participant frame.
pub const CANCEL_BY_PARTICIPANT_PAYLOAD_LEN: usize = 16;
/// Payload width of an amend frame.
pub const AMEND_PAYLOAD_LEN: usize = 32;

/// One accepted book command, mirroring the four `Book` command
/// methods field for field.
///
/// The WAL persists commands, never events, and a rejected command
/// is never written, so every frame in a log is a command the book
/// accepted at its sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalCommand {
    /// Add an order, as `Book::add_order`.
    Add {
        /// Caller-supplied order identifier.
        order_id: OrderId,
        /// The order's owner.
        participant: ParticipantId,
        /// Which side of the book.
        side: Side,
        /// Order type with time in force folded in.
        order_type: OrderType,
        /// Self-trade prevention policy, or `None` for the book
        /// default.
        stp: Option<StpPolicy>,
        /// Raw price in quote units.
        price: u64,
        /// Quantity.
        qty: u64,
    },
    /// Cancel one live order, as `Book::cancel_order`.
    Cancel {
        /// The order to cancel.
        order_id: OrderId,
    },
    /// Cancel every live order of one participant, as
    /// `Book::cancel_by_participant`.
    CancelByParticipant {
        /// The participant whose orders are cancelled.
        participant: ParticipantId,
    },
    /// Amend one live order, as `Book::amend_order`.
    Amend {
        /// The order to amend.
        order_id: OrderId,
        /// New raw price in quote units.
        new_price: u64,
        /// New remaining quantity.
        new_qty: u64,
    },
}

impl WalCommand {
    /// The frame kind byte for this command.
    #[must_use]
    pub const fn kind(&self) -> u8 {
        match self {
            Self::Add { .. } => KIND_ADD,
            Self::Cancel { .. } => KIND_CANCEL,
            Self::CancelByParticipant { .. } => KIND_CANCEL_BY_PARTICIPANT,
            Self::Amend { .. } => KIND_AMEND,
        }
    }

    /// The fixed payload width of this command's frame kind.
    #[must_use]
    pub const fn payload_len(&self) -> usize {
        match self {
            Self::Add { .. } => ADD_PAYLOAD_LEN,
            Self::Cancel { .. } => CANCEL_PAYLOAD_LEN,
            Self::CancelByParticipant { .. } => CANCEL_BY_PARTICIPANT_PAYLOAD_LEN,
            Self::Amend { .. } => AMEND_PAYLOAD_LEN,
        }
    }

    /// Run this command against a book, exactly as the caller would
    /// have at write time. Recovery replays frames through here.
    ///
    /// # Errors
    ///
    /// Returns the [`BookError`] the book returns, unchanged.
    pub fn apply(&self, book: &mut Book, events: &mut EventBuffer<'_>) -> Result<usize, BookError> {
        match *self {
            Self::Add {
                order_id,
                participant,
                side,
                order_type,
                stp,
                price,
                qty,
            } => book.add_order(
                order_id,
                participant,
                side,
                order_type,
                price,
                qty,
                stp,
                events,
            ),
            Self::Cancel { order_id } => book.cancel_order(order_id, events),
            Self::CancelByParticipant { participant } => {
                book.cancel_by_participant(participant, events)
            }
            Self::Amend {
                order_id,
                new_price,
                new_qty,
            } => book.amend_order(order_id, new_price, new_qty, events),
        }
    }
}

/// Encode one frame for `command` at sequence `seq` into `out`,
/// clearing any previous content.
///
/// The buffer is reused across calls: after the first frame of the
/// largest kind, its capacity never changes again (INV-ALLOC-3).
pub fn encode_frame(command: &WalCommand, seq: u64, out: &mut Vec<u8>) {
    let frame_len = HEADER_LEN + command.payload_len();
    out.clear();
    out.resize(frame_len, 0);
    out[0..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&32u16.to_le_bytes());
    out[8..16].copy_from_slice(&seq.to_le_bytes());
    let payload_len = u32::try_from(command.payload_len()).unwrap_or(0);
    out[16..20].copy_from_slice(&payload_len.to_le_bytes());
    out[20] = command.kind();
    // Bytes 21..28 (flags, reserved, reserved2) stay zero from the
    // resize; the checksum overwrites bytes 28..32 below.
    let payload = &mut out[HEADER_LEN..frame_len];
    match *command {
        WalCommand::Add {
            order_id,
            participant,
            side,
            order_type,
            stp,
            price,
            qty,
        } => {
            payload[0..8].copy_from_slice(&order_id.0.to_le_bytes());
            payload[8..12].copy_from_slice(&participant.0.to_le_bytes());
            payload[12] = side_to_byte(side);
            payload[13] = order_type_to_byte(order_type);
            payload[14] = stp.map_or(255, stp_to_byte);
            payload[16..24].copy_from_slice(&price.to_le_bytes());
            payload[24..32].copy_from_slice(&qty.to_le_bytes());
        }
        WalCommand::Cancel { order_id } => {
            payload[0..8].copy_from_slice(&order_id.0.to_le_bytes());
        }
        WalCommand::CancelByParticipant { participant } => {
            payload[0..4].copy_from_slice(&participant.0.to_le_bytes());
        }
        WalCommand::Amend {
            order_id,
            new_price,
            new_qty,
        } => {
            payload[0..8].copy_from_slice(&order_id.0.to_le_bytes());
            payload[8..16].copy_from_slice(&new_price.to_le_bytes());
            payload[16..24].copy_from_slice(&new_qty.to_le_bytes());
        }
    }
    let crc = frame_crc(&out[0..28], &out[HEADER_LEN..frame_len]);
    out[28..32].copy_from_slice(&crc.to_le_bytes());
}

/// The outcome of decoding at one position in a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameParse {
    /// A complete, valid frame.
    Frame {
        /// The frame's authoritative sequence number.
        seq: u64,
        /// The decoded command.
        command: WalCommand,
        /// Total frame length in bytes, header plus payload.
        frame_len: usize,
    },
    /// The remaining bytes are the torn tail of a frame a crash
    /// interrupted (section 9.5). `discarded` counts them.
    CrashBoundary {
        /// Trailing bytes to discard.
        discarded: usize,
    },
}

/// Decode the frame starting at the front of `bytes`, which is the
/// remaining tail of a log whose first byte sits at absolute offset
/// `offset`. Implements the decision table of section 9.5:
///
/// - A trailing fragment shorter than a header is a clean crash
///   boundary when every fixed-value byte present matches a valid
///   header prefix, and corruption at `offset` when any does not.
/// - A complete header with a short payload is a torn write and a
///   clean crash boundary, once the header itself validates.
/// - A complete header with an invalid field (version, header
///   length, unknown kind, payload length over the cap or off its
///   kind's fixed width, any reserved field nonzero) is corruption
///   at `offset`.
/// - A complete frame whose checksum does not match is corruption
///   at `offset`, never a crash boundary.
///
/// # Errors
///
/// Returns [`WalError::CorruptWalRecord`] with the offset and the
/// specific failure.
pub fn parse_frame(bytes: &[u8], offset: u64) -> Result<FrameParse, WalError> {
    if bytes.len() < HEADER_LEN {
        return fragment_verdict(bytes, offset);
    }
    let header = &bytes[..HEADER_LEN];
    if header[0..4] != MAGIC {
        return Err(corrupt(offset, WalCorruption::BadMagic));
    }
    let version = read_u16(header, 4);
    if version != VERSION {
        return Err(corrupt(
            offset,
            WalCorruption::BadVersion { found: version },
        ));
    }
    let header_len = read_u16(header, 6);
    if header_len != 32 {
        return Err(corrupt(
            offset,
            WalCorruption::BadHeaderLen { found: header_len },
        ));
    }
    let payload_len = read_u32(header, 16);
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(corrupt(
            offset,
            WalCorruption::PayloadTooLarge { found: payload_len },
        ));
    }
    let kind = header[20];
    let width = match kind {
        KIND_ADD => ADD_PAYLOAD_LEN,
        KIND_CANCEL => CANCEL_PAYLOAD_LEN,
        KIND_CANCEL_BY_PARTICIPANT => CANCEL_BY_PARTICIPANT_PAYLOAD_LEN,
        KIND_AMEND => AMEND_PAYLOAD_LEN,
        _ => {
            return Err(corrupt(offset, WalCorruption::UnknownKind { found: kind }));
        }
    };
    if payload_len as usize != width {
        return Err(corrupt(
            offset,
            WalCorruption::PayloadLenMismatch {
                kind,
                expected: u32::try_from(width).unwrap_or(0),
                found: payload_len,
            },
        ));
    }
    if header[21] != 0 {
        return Err(corrupt(
            offset,
            WalCorruption::NonZeroReserved { field: "flags" },
        ));
    }
    if header[22..24] != [0u8; 2] {
        return Err(corrupt(
            offset,
            WalCorruption::NonZeroReserved { field: "reserved" },
        ));
    }
    if header[24..28] != [0u8; 4] {
        return Err(corrupt(
            offset,
            WalCorruption::NonZeroReserved { field: "reserved2" },
        ));
    }
    let frame_len = HEADER_LEN + width;
    if bytes.len() < frame_len {
        // A valid header whose payload never fully arrived: a torn
        // write, reported as a crash boundary (section 9.5).
        return Ok(FrameParse::CrashBoundary {
            discarded: bytes.len(),
        });
    }
    let payload = &bytes[HEADER_LEN..frame_len];
    let stored_crc = read_u32(header, 28);
    let computed_crc = frame_crc(&header[0..28], payload);
    if computed_crc != stored_crc {
        return Err(corrupt(
            offset,
            WalCorruption::CrcMismatch {
                expected_crc: computed_crc,
                actual_crc: stored_crc,
            },
        ));
    }
    let seq = read_u64(header, 8);
    let command = decode_payload(kind, payload, offset)?;
    Ok(FrameParse::Frame {
        seq,
        command,
        frame_len,
    })
}

/// The verdict for a trailing fragment shorter than a header
/// (section 9.5): a clean crash boundary when every fixed-value byte
/// present matches a valid header prefix, corruption at the fragment
/// start when any one does not. Sequence, payload length, and
/// checksum positions constrain nothing in a fragment.
fn fragment_verdict(bytes: &[u8], offset: u64) -> Result<FrameParse, WalError> {
    for (i, &byte) in bytes.iter().enumerate() {
        let mismatch = match i {
            0..=3 => byte != MAGIC[i],
            4 => byte != 1,
            5 | 7 | 21..=27 => byte != 0,
            6 => byte != 32,
            20 => !matches!(
                byte,
                KIND_ADD | KIND_CANCEL | KIND_CANCEL_BY_PARTICIPANT | KIND_AMEND
            ),
            _ => false,
        };
        if mismatch {
            return Err(corrupt(offset, fragment_detail(i, byte, bytes)));
        }
    }
    Ok(FrameParse::CrashBoundary {
        discarded: bytes.len(),
    })
}

/// The corruption detail for one mismatched fixed-value byte inside
/// a fragment, naming the field the byte belongs to.
fn fragment_detail(position: usize, byte: u8, bytes: &[u8]) -> WalCorruption {
    match position {
        0..=3 => WalCorruption::BadMagic,
        4..=5 => WalCorruption::BadVersion {
            found: partial_u16(bytes, 4),
        },
        6..=7 => WalCorruption::BadHeaderLen {
            found: partial_u16(bytes, 6),
        },
        20 => WalCorruption::UnknownKind { found: byte },
        21 => WalCorruption::NonZeroReserved { field: "flags" },
        22..=23 => WalCorruption::NonZeroReserved { field: "reserved" },
        _ => WalCorruption::NonZeroReserved { field: "reserved2" },
    }
}

/// A u16 read from a fragment that may hold only its low byte.
fn partial_u16(bytes: &[u8], at: usize) -> u16 {
    let low = u16::from(bytes[at]);
    match bytes.get(at + 1) {
        Some(&high) => low | (u16::from(high) << 8),
        None => low,
    }
}

/// Decode one payload of a known kind, checking its reserved fields
/// and the defined value sets of its coded fields.
fn decode_payload(kind: u8, payload: &[u8], offset: u64) -> Result<WalCommand, WalError> {
    match kind {
        KIND_ADD => {
            if payload[15] != 0 {
                return Err(corrupt(
                    offset,
                    WalCorruption::NonZeroReserved {
                        field: "add.reserved",
                    },
                ));
            }
            if payload[32..40] != [0u8; 8] {
                return Err(corrupt(
                    offset,
                    WalCorruption::NonZeroReserved {
                        field: "add.reserved_tail",
                    },
                ));
            }
            let side = match payload[12] {
                0 => Side::Bid,
                1 => Side::Ask,
                _ => {
                    return Err(corrupt(
                        offset,
                        WalCorruption::BadPayloadField { field: "add.side" },
                    ));
                }
            };
            let order_type = match payload[13] {
                0 => OrderType::Limit,
                1 => OrderType::Ioc,
                2 => OrderType::Fok,
                3 => OrderType::PostOnly,
                _ => {
                    return Err(corrupt(
                        offset,
                        WalCorruption::BadPayloadField {
                            field: "add.order_type",
                        },
                    ));
                }
            };
            let stp = match payload[14] {
                0 => Some(StpPolicy::CancelIncoming),
                1 => Some(StpPolicy::CancelResting),
                2 => Some(StpPolicy::CancelBoth),
                255 => None,
                _ => {
                    return Err(corrupt(
                        offset,
                        WalCorruption::BadPayloadField { field: "add.stp" },
                    ));
                }
            };
            Ok(WalCommand::Add {
                order_id: OrderId(read_u64(payload, 0)),
                participant: ParticipantId(read_u32(payload, 8)),
                side,
                order_type,
                stp,
                price: read_u64(payload, 16),
                qty: read_u64(payload, 24),
            })
        }
        KIND_CANCEL => {
            if payload[8..16] != [0u8; 8] {
                return Err(corrupt(
                    offset,
                    WalCorruption::NonZeroReserved {
                        field: "cancel.reserved",
                    },
                ));
            }
            Ok(WalCommand::Cancel {
                order_id: OrderId(read_u64(payload, 0)),
            })
        }
        KIND_CANCEL_BY_PARTICIPANT => {
            if payload[4..16] != [0u8; 12] {
                return Err(corrupt(
                    offset,
                    WalCorruption::NonZeroReserved {
                        field: "cancel_by_participant.reserved",
                    },
                ));
            }
            Ok(WalCommand::CancelByParticipant {
                participant: ParticipantId(read_u32(payload, 0)),
            })
        }
        _ => {
            // KIND_AMEND is the only remaining known kind.
            if payload[24..32] != [0u8; 8] {
                return Err(corrupt(
                    offset,
                    WalCorruption::NonZeroReserved {
                        field: "amend.reserved",
                    },
                ));
            }
            Ok(WalCommand::Amend {
                order_id: OrderId(read_u64(payload, 0)),
                new_price: read_u64(payload, 8),
                new_qty: read_u64(payload, 16),
            })
        }
    }
}

/// Shorthand for a corruption error at one offset.
fn corrupt(offset: u64, detail: WalCorruption) -> WalError {
    WalError::CorruptWalRecord { offset, detail }
}

/// Little-endian u16 at byte `at`; the caller guarantees the width.
fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(
        bytes[at..at + 2]
            .try_into()
            .expect("caller guarantees two bytes"),
    )
}

/// Little-endian u32 at byte `at`; the caller guarantees the width.
fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        bytes[at..at + 4]
            .try_into()
            .expect("caller guarantees four bytes"),
    )
}

/// Little-endian u64 at byte `at`; the caller guarantees the width.
fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(
        bytes[at..at + 8]
            .try_into()
            .expect("caller guarantees eight bytes"),
    )
}

/// Side encoding in frame payloads: 0 bid, 1 ask.
pub const fn side_to_byte(side: Side) -> u8 {
    match side {
        Side::Bid => 0,
        Side::Ask => 1,
    }
}

/// Order type encoding in frame payloads.
pub const fn order_type_to_byte(order_type: OrderType) -> u8 {
    match order_type {
        OrderType::Limit => 0,
        OrderType::Ioc => 1,
        OrderType::Fok => 2,
        OrderType::PostOnly => 3,
    }
}

/// Self-trade prevention encoding in frame payloads and the snapshot
/// header: 0 cancel incoming, 1 cancel resting, 2 cancel both. (255
/// in an add payload means the book default applies.)
pub const fn stp_to_byte(stp: StpPolicy) -> u8 {
    match stp {
        StpPolicy::CancelIncoming => 0,
        StpPolicy::CancelResting => 1,
        StpPolicy::CancelBoth => 2,
    }
}

/// Decode a side byte, or `None` outside the defined set.
pub const fn side_from_byte(byte: u8) -> Option<Side> {
    match byte {
        0 => Some(Side::Bid),
        1 => Some(Side::Ask),
        _ => None,
    }
}

/// Decode a self-trade prevention byte, or `None` outside the
/// defined set.
pub const fn stp_from_byte(byte: u8) -> Option<StpPolicy> {
    match byte {
        0 => Some(StpPolicy::CancelIncoming),
        1 => Some(StpPolicy::CancelResting),
        2 => Some(StpPolicy::CancelBoth),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc::snapshot_crc;

    fn sample_add() -> WalCommand {
        WalCommand::Add {
            order_id: OrderId(1),
            participant: ParticipantId(7),
            side: Side::Ask,
            order_type: OrderType::Limit,
            stp: None,
            price: 100,
            qty: 10,
        }
    }

    #[test]
    fn wal_frame_golden_vectors() {
        // INV-WAL-1: the add frame for seq 1, byte for byte except the
        // checksum word, which is pinned by the independent check
        // value in the crc module tests plus recomputation here.
        let mut out = Vec::new();
        encode_frame(&sample_add(), 1, &mut out);
        let expected_header: [u8; 28] = [
            0x43, 0x57, 0x41, 0x4C, // CWAL
            1, 0, // version 1
            32, 0, // header length 32
            1, 0, 0, 0, 0, 0, 0, 0, // seq 1
            40, 0, 0, 0, // payload length 40
            1, // kind add
            0, // flags
            0, 0, // reserved
            0, 0, 0, 0, // reserved2
        ];
        assert_eq!(out.len(), 72);
        assert_eq!(&out[0..28], &expected_header[..]);
        let expected_payload: [u8; 40] = [
            1, 0, 0, 0, 0, 0, 0, 0, // order id 1
            7, 0, 0, 0,   // participant 7
            1,   // side ask
            0,   // type limit
            255, // stp: book default
            0,   // reserved
            100, 0, 0, 0, 0, 0, 0, 0, // price 100
            10, 0, 0, 0, 0, 0, 0, 0, // qty 10
            0, 0, 0, 0, 0, 0, 0, 0, // reserved
        ];
        assert_eq!(&out[32..72], &expected_payload[..]);
        let stored = u32::from_le_bytes([out[28], out[29], out[30], out[31]]);
        assert_eq!(stored, frame_crc(&out[0..28], &out[32..72]));

        // Cancel frame: kind 2, 16 byte payload.
        let mut out = Vec::new();
        encode_frame(
            &WalCommand::Cancel {
                order_id: OrderId(9),
            },
            2,
            &mut out,
        );
        assert_eq!(out.len(), 48);
        assert_eq!(out[20], KIND_CANCEL);
        assert_eq!(&out[32..40], &9u64.to_le_bytes());
        assert_eq!(&out[40..48], &[0u8; 8]);

        // Cancel by participant frame: kind 3, 16 byte payload.
        let mut out = Vec::new();
        encode_frame(
            &WalCommand::CancelByParticipant {
                participant: ParticipantId(3),
            },
            3,
            &mut out,
        );
        assert_eq!(out.len(), 48);
        assert_eq!(out[20], KIND_CANCEL_BY_PARTICIPANT);
        assert_eq!(&out[32..36], &3u32.to_le_bytes());
        assert_eq!(&out[36..48], &[0u8; 12]);

        // Amend frame: kind 4, 32 byte payload.
        let mut out = Vec::new();
        encode_frame(
            &WalCommand::Amend {
                order_id: OrderId(5),
                new_price: 99,
                new_qty: 2,
            },
            4,
            &mut out,
        );
        assert_eq!(out.len(), 64);
        assert_eq!(out[20], KIND_AMEND);
        assert_eq!(&out[32..40], &5u64.to_le_bytes());
        assert_eq!(&out[40..48], &99u64.to_le_bytes());
        assert_eq!(&out[48..56], &2u64.to_le_bytes());
        assert_eq!(&out[56..64], &[0u8; 8]);
    }

    #[test]
    fn wal_crc_domain_separation() {
        // INV-WAL-3: a frame checksum recomputed under the snapshot
        // domain prefix does not equal the stored frame checksum.
        let mut out = Vec::new();
        encode_frame(&sample_add(), 1, &mut out);
        let stored = u32::from_le_bytes([out[28], out[29], out[30], out[31]]);
        let mut concat = out[0..28].to_vec();
        concat.extend_from_slice(&out[32..72]);
        assert_ne!(snapshot_crc(&concat), stored);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let commands = [
            (sample_add(), 1u64),
            (
                WalCommand::Cancel {
                    order_id: OrderId(9),
                },
                2,
            ),
            (
                WalCommand::CancelByParticipant {
                    participant: ParticipantId(3),
                },
                3,
            ),
            (
                WalCommand::Amend {
                    order_id: OrderId(5),
                    new_price: 99,
                    new_qty: 2,
                },
                4,
            ),
            (
                WalCommand::Add {
                    order_id: OrderId(6),
                    participant: ParticipantId(1),
                    side: Side::Bid,
                    order_type: OrderType::PostOnly,
                    stp: Some(StpPolicy::CancelBoth),
                    price: 55,
                    qty: 1,
                },
                5,
            ),
        ];
        let mut buf = Vec::new();
        for (command, seq) in commands {
            encode_frame(&command, seq, &mut buf);
            let parsed = parse_frame(&buf, 0).expect("a written frame decodes");
            assert_eq!(
                parsed,
                FrameParse::Frame {
                    seq,
                    command,
                    frame_len: buf.len(),
                }
            );
        }
    }
}
