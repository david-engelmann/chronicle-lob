//! chronicle-core: a single-threaded, deterministic limit order book and
//! matching engine with zero steady-state heap allocation.
//!
//! The design contract is `docs/ARCHITECTURE.md` in the repository. The
//! short version:
//!
//! - Orders live in a fixed-capacity struct-of-arrays arena and are
//!   named only by [`OrderId`]. Raw slot handles never leave this crate.
//! - Prices sit on a bounded tick grid. Active levels are tracked by
//!   hierarchical bitmaps, so best bid and best ask cost a few word
//!   reads, independent of how many levels are live.
//! - An open-addressed table with backward-shift deletion maps
//!   [`OrderId`] to arena slots at a load factor of at most 50 percent.
//! - Every mutating command validates and preflights before it mutates,
//!   so every rejection is atomic. Execution events are written into a
//!   caller-owned buffer; a buffer that is too small is a typed error
//!   returned before any state changes.
//!
//! This crate is strictly std-only, forbids unsafe code, reads no clock,
//! and uses no process-seeded randomness. It is the deterministic core
//! that `chronicle-wal` persists and `chronicle-cli` drives.
#![forbid(unsafe_code)]

pub(crate) mod arena;
pub(crate) mod bitmap;
pub(crate) mod book;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod events;
pub(crate) mod level;
pub(crate) mod lookup;
pub(crate) mod types;

#[cfg(test)]
mod prop_tests;
#[cfg(test)]
mod reference_book;

pub use book::{Book, DepthLevel};
pub use config::BookConfig;
pub use error::BookError;
pub use events::{CancelReason, EventBuffer, ExecutionEvent};
pub use types::{OrderId, OrderType, ParticipantId, Side, StpPolicy};
