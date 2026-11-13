//! Execution events and the caller-owned event buffer.
//!
//! Events carry only the sequence number of their command, order ids,
//! prices, and quantities (ARCHITECTURE.md section 7.6). No slot
//! indices, no timestamps, no pointers: event streams are comparable
//! byte for byte across processes and across snapshot restore.

use crate::types::OrderId;

/// Why an order was cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancelReason {
    /// Explicit cancel command from the caller.
    Requested,
    /// The participant cancelled all of its orders in one command.
    ByParticipant,
    /// Self-trade prevention cancelled this resting order.
    SelfTradePrevention,
}

/// One execution event. Every variant carries the `seq` of the command
/// that produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionEvent {
    /// An incoming order rested its remainder on the book.
    Accepted {
        /// Command sequence number.
        seq: u64,
        /// The order that rested.
        order_id: OrderId,
    },
    /// A trade between a resting maker and the incoming taker, at the
    /// maker price.
    Trade {
        /// Command sequence number.
        seq: u64,
        /// The resting order.
        maker_order_id: OrderId,
        /// The incoming order.
        taker_order_id: OrderId,
        /// Trade price in quote units (the maker price).
        price: u64,
        /// Traded quantity.
        qty: u64,
    },
    /// An order reached zero remaining quantity by trading.
    Filled {
        /// Command sequence number.
        seq: u64,
        /// The order that filled.
        order_id: OrderId,
    },
    /// An order left the book by cancellation.
    Cancelled {
        /// Command sequence number.
        seq: u64,
        /// The cancelled order.
        order_id: OrderId,
        /// Why it was cancelled.
        reason: CancelReason,
    },
    /// An order was amended. Price and quantity are the new values.
    Amended {
        /// Command sequence number.
        seq: u64,
        /// The amended order.
        order_id: OrderId,
        /// New raw price in quote units.
        price: u64,
        /// New remaining quantity.
        qty: u64,
    },
    /// An immediate-or-cancel order remainder expired unfilled.
    Expired {
        /// Command sequence number.
        seq: u64,
        /// The expired order.
        order_id: OrderId,
        /// Quantity that did not fill.
        remaining_qty: u64,
    },
    /// Self-trade prevention stopped the incoming order at its own
    /// resting order.
    SelfTradePrevented {
        /// Command sequence number.
        seq: u64,
        /// The incoming order whose remainder was cancelled.
        order_id: OrderId,
    },
}

/// Caller-owned, pre-allocated event sink for one or more commands.
///
/// The buffer borrows a `Vec<ExecutionEvent>` the caller has already
/// reserved capacity on. The engine checks the remaining capacity in
/// preflight against the command's worst-case event count and returns
/// [`crate::BookError::EventBufferFull`] before mutating anything when
/// it does not fit, so a `push` inside the engine can never grow the
/// vector. Events written by one command are the tail slice reported by
/// [`EventBuffer::written`].
pub struct EventBuffer<'a> {
    out: &'a mut Vec<ExecutionEvent>,
    start: usize,
}

impl<'a> EventBuffer<'a> {
    /// Wrap a caller vector. Events already in the vector are left
    /// alone; capacity beyond its current length is the space commands
    /// may use.
    #[must_use]
    pub const fn new(out: &'a mut Vec<ExecutionEvent>) -> Self {
        let start = out.len();
        Self { out, start }
    }

    /// Free event slots: reserved capacity minus current length.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.out.capacity() - self.out.len()
    }

    /// Events written since this buffer was created.
    #[must_use]
    pub fn written(&self) -> &[ExecutionEvent] {
        &self.out[self.start..]
    }

    /// Append one event. Engine-internal: preflight has already proved
    /// the capacity suffices, so this never reallocates.
    pub(crate) fn emit(&mut self, event: ExecutionEvent) {
        debug_assert!(self.remaining() > 0, "emit beyond preflight bound");
        self.out.push(event);
    }
}
