//! INV-ALLOC-3, the counter proof for the WAL writer: with a
//! thread-local counting allocator, appends to a warmed writer
//! perform zero heap allocations in steady state. The counting
//! allocator lives in this dedicated test target, the one place the
//! workspace permits unsafe code, and counts only the thread that
//! installs it per test, so parallel tests cannot pollute the count.
#![allow(
    unsafe_code,
    reason = "a counting GlobalAlloc is inherently unsafe; it is confined to this test target and only forwards to the system allocator"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io;

use chronicle_core::{OrderId, OrderType, ParticipantId, Side};
use chronicle_wal::{SyncPolicy, WalCommand, WalSink, WalWriter};

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// A global allocator that counts allocations per thread and
/// forwards everything to the system allocator.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        // SAFETY: forwards the caller's layout to the system
        // allocator unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwards a pointer and layout this allocator
        // produced, as the GlobalAlloc contract requires of callers.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// A sink that discards bytes and never allocates.
struct NullSink;

impl io::Write for NullSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl WalSink for NullSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn alloc_wal_append_zero_steady_state() {
    let command = WalCommand::Add {
        order_id: OrderId(1),
        participant: ParticipantId(1),
        side: Side::Bid,
        order_type: OrderType::Limit,
        stp: None,
        price: 100,
        qty: 1,
    };
    let mut writer = WalWriter::new(NullSink, SyncPolicy::Never);
    writer.append(&command, 1).expect("warm the encode buffer");
    let before = ALLOCATIONS.with(Cell::get);
    for seq in 2..=1_000u64 {
        writer.append(&command, seq).expect("append");
    }
    let after = ALLOCATIONS.with(Cell::get);
    assert_eq!(
        after - before,
        0,
        "999 steady-state appends must not allocate"
    );
}
