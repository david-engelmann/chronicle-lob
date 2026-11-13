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
/// 3.3). Milestone 1 executes [`OrderType::Limit`]; the other variants
/// are part of the locked type contract and execute from Milestone 2.
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
/// book-level default (ARCHITECTURE.md section 7.5). Executes from
/// Milestone 2; defined now because it is part of `BookConfig`.
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
