//! Domain errors. Every variant carries the concrete values that explain
//! the failure (ARCHITECTURE.md section 8.1): an opaque variant name is
//! never the whole diagnostic.

use core::fmt;

use crate::types::{OrderId, OrderType};

/// Errors returned by book construction and by every book command.
///
/// Commands that return an error have mutated nothing: no order, level
/// aggregate, bitmap bit, lookup entry, or sequence number changes when
/// any of these is returned (INV-ORD-4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookError {
    /// `BookConfig` failed validation at construction.
    InvalidConfig {
        /// The field that failed validation.
        field: &'static str,
        /// The rule the field violated.
        rule: &'static str,
    },
    /// An order id of zero was supplied; zero is the lookup sentinel.
    InvalidOrderId {
        /// The offending order id (always zero).
        order_id: OrderId,
    },
    /// An active order already uses this id.
    DuplicateOrderId {
        /// The conflicting order id.
        order_id: OrderId,
    },
    /// A quantity of zero was supplied on add or amend.
    InvalidQuantity {
        /// The offending quantity (always zero).
        qty: u64,
    },
    /// A price failed the tick grid rules of ARCHITECTURE.md section 3.2.
    InvalidPriceTick {
        /// The raw price supplied, in quote units.
        price: u64,
        /// Configured quote units per tick.
        tick_size: u64,
        /// Configured tick value of grid index 0.
        grid_start_tick: u64,
        /// Configured number of ticks on the grid.
        grid_len: u32,
    },
    /// Cancel or amend named an order that is not live.
    OrderNotFound {
        /// The order id that was not found.
        order_id: OrderId,
    },
    /// The arena has no free slot for an order that would rest.
    CapacityFull {
        /// Configured arena capacity.
        max_orders: u32,
    },
    /// The lookup table reached its 50 percent load cap. Defensive: the
    /// arena cap is enforced first, so this guards the table invariant
    /// directly rather than relying on that ordering.
    LookupCapacityFull {
        /// Table capacity in buckets.
        capacity: usize,
        /// Entries present when the insert was refused.
        len: usize,
    },
    /// A Post-Only order price would cross the opposite best price.
    PostOnlyWouldCross {
        /// The raw price of the incoming order.
        price: u64,
        /// The raw opposite best price it would have crossed.
        opposite_best: u64,
    },
    /// A Fill-or-Kill order could not be filled in full.
    FokInsufficientLiquidity {
        /// Quantity the order requested.
        requested: u64,
        /// STP-adjusted executable quantity found in preflight.
        available: u64,
    },
    /// An amend would leave the book crossed.
    AmendWouldCross {
        /// The order being amended.
        order_id: OrderId,
        /// The raw new price requested.
        new_price: u64,
        /// The raw opposite best price it would cross.
        opposite_best: u64,
    },
    /// The caller event buffer is smaller than the preflight worst-case
    /// event count for this command. Returned before any mutation, so
    /// the identical command can be retried with a larger buffer.
    EventBufferFull {
        /// Worst-case events the command could emit.
        needed: usize,
        /// Free slots the buffer actually had.
        available: usize,
    },
    /// A checked aggregate operation overflowed. Defensive guard:
    /// aggregates are sized so this is unreachable in a valid book.
    ArithmeticOverflow {
        /// What was being computed when the overflow was detected.
        what: &'static str,
    },
    /// The order type is part of the locked contract but its execution
    /// lands in a later milestone (Milestone 2). Milestone 1 executes
    /// Limit orders only.
    UnsupportedOrderType {
        /// The order type that was supplied.
        order_type: OrderType,
    },
}

impl fmt::Display for BookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { field, rule } => {
                write!(
                    f,
                    "invalid book configuration: field {field} violates rule: {rule}"
                )
            }
            Self::InvalidOrderId { order_id } => {
                write!(f, "invalid order id {}: zero is reserved", order_id.0)
            }
            Self::DuplicateOrderId { order_id } => {
                write!(
                    f,
                    "duplicate order id {}: an active order already uses it",
                    order_id.0
                )
            }
            Self::InvalidQuantity { qty } => {
                write!(f, "invalid quantity {qty}: quantities must be positive")
            }
            Self::InvalidPriceTick {
                price,
                tick_size,
                grid_start_tick,
                grid_len,
            } => write!(
                f,
                "invalid price tick: price {price} with tick size {tick_size} does not land on the grid starting at tick {grid_start_tick} with {grid_len} ticks"
            ),
            Self::OrderNotFound { order_id } => {
                write!(f, "order not found: no live order with id {}", order_id.0)
            }
            Self::CapacityFull { max_orders } => {
                write!(
                    f,
                    "book capacity full: arena holds at most {max_orders} live orders"
                )
            }
            Self::LookupCapacityFull { capacity, len } => write!(
                f,
                "lookup table at load cap: {len} entries in {capacity} buckets (cap is 50 percent)"
            ),
            Self::PostOnlyWouldCross {
                price,
                opposite_best,
            } => write!(
                f,
                "post-only order at price {price} would cross the opposite best price {opposite_best}"
            ),
            Self::FokInsufficientLiquidity {
                requested,
                available,
            } => write!(
                f,
                "fill-or-kill order requested {requested} but only {available} is executable"
            ),
            Self::AmendWouldCross {
                order_id,
                new_price,
                opposite_best,
            } => write!(
                f,
                "amend of order {} to price {new_price} would cross the opposite best price {opposite_best}",
                order_id.0
            ),
            Self::EventBufferFull { needed, available } => write!(
                f,
                "event buffer full: command needs up to {needed} event slots, buffer has {available}"
            ),
            Self::ArithmeticOverflow { what } => {
                write!(f, "arithmetic overflow while computing {what}")
            }
            Self::UnsupportedOrderType { order_type } => {
                write!(
                    f,
                    "order type {order_type:?} is not executed in this milestone"
                )
            }
        }
    }
}

impl core::error::Error for BookError {}
