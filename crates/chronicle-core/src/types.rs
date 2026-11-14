//! Core value types. Prices and quantities travel as plain `u64` values
//! at the API boundary (prices in the smallest quote unit, validated
//! against the tick grid on entry); identifiers are newtypes so an
//! order id can never be confused with a participant id at a call site.

/// Caller-supplied order identifier. Zero is reserved as the empty
/// sentinel of the lookup table and is rejected as
/// [`crate::BookError::InvalidOrderId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub u64);

/// Caller-supplied participant identifier. Zero is a valid participant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParticipantId(pub u32);

/// Which side of the book an order belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// Buy side. Bids rest below asks; the best bid is the highest price.
    Bid,
    /// Sell side. The best ask is the lowest price.
    Ask,
}

impl Side {
    /// The opposite side, which an incoming order on this side matches
    /// against.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }

    /// Arena column encoding: 0 for bid, 1 for ask.
    #[must_use]
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Self::Bid => 0,
            Self::Ask => 1,
        }
    }

    /// Decode an arena column value written by [`Side::as_u8`].
    #[must_use]
    pub(crate) const fn from_u8(value: u8) -> Self {
        if value == 0 { Self::Bid } else { Self::Ask }
    }
}

/// Order type, with time in force folded in (ARCHITECTURE.md section
/// 3.3). Every variant executes; the types differ only in the
/// section 7.3 remainder rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OrderType {
    /// Rest any unfilled remainder on the book.
    Limit,
    /// Immediate or cancel: fill what is available now, expire the rest.
    Ioc,
    /// Fill or kill: fill the full quantity now or nothing at all.
    Fok,
    /// Post only: rest, but reject if the price would cross.
    PostOnly,
}

/// Self-trade prevention policy, carried by the incoming order with a
/// book-level default (ARCHITECTURE.md section 7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StpPolicy {
    /// Cancel the incoming order remainder at the first own resting
    /// order it would trade with.
    CancelIncoming,
    /// Cancel the resting own order and continue the sweep.
    CancelResting,
    /// Cancel both the resting own order and the incoming remainder.
    CancelBoth,
}

/// Structural state of one live order, as persisted in a snapshot
/// (ARCHITECTURE.md section 10.2, live slot records). Slot indices and
/// links are raw `u32` arena indices; `prev` and `next` are
/// `u32::MAX` (the arena NIL) at the ends of a level list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveOrderState {
    /// Arena slot the order occupies.
    pub slot: u32,
    /// Generation of the slot at capture time.
    pub generation: u32,
    /// The order's caller-supplied id.
    pub order_id: OrderId,
    /// The order's owner.
    pub participant: ParticipantId,
    /// Dense tick index of the order's price level.
    pub price_idx: u32,
    /// Remaining quantity.
    pub qty: u64,
    /// Which side the order rests on.
    pub side: Side,
    /// Previous slot in the level list, or NIL.
    pub prev: u32,
    /// Next slot in the level list, or NIL.
    pub next: u32,
}

/// One price level's persisted state (ARCHITECTURE.md section 10.2,
/// level arrays): list ends, aggregate quantity, and order count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelState {
    /// First slot at the level, or NIL when the level is empty.
    pub head: u32,
    /// Last slot at the level, or NIL when the level is empty.
    pub tail: u32,
    /// Sum of remaining quantities at the level.
    pub qty: u64,
    /// Number of live orders at the level.
    pub count: u32,
}

/// The complete structural state of a [`crate::Book`], captured for
/// persistence by `chronicle-wal` (ARCHITECTURE.md section 10). This
/// is a plain data transfer shape: the byte format, its validation,
/// and its CRC live in `chronicle-wal`; the book only captures and
/// restores its own internals through it, so a restore reproduces
/// slot assignment, FIFO links, generations, and the exact free list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotState {
    /// The configuration the book was built with.
    pub config: crate::config::BookConfig,
    /// Sequence number the next accepted command will take.
    pub next_seq: u64,
    /// Generation of every slot, free, live, and retired alike, in
    /// slot order. Length is `config.max_orders`.
    pub generations: Vec<u32>,
    /// Head of the free list in allocation order, or NIL when no slot
    /// is free.
    pub free_list_head: u32,
    /// Per-slot free list link, in slot order: for a free slot, the
    /// next free slot in allocation order or NIL at the end of the
    /// chain; NIL for live and retired slots. Length is
    /// `config.max_orders`.
    pub free_links: Vec<u32>,
    /// Per-level state in tick index order. Length is
    /// `config.grid_len`.
    pub levels: Vec<LevelState>,
    /// Every live order, in ascending slot order.
    pub live_orders: Vec<LiveOrderState>,
}
