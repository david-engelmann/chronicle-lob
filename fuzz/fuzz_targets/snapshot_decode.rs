#![no_main]

use chronicle_core::{Book, BookConfig, StpPolicy};
use chronicle_wal::{decode_snapshot, snapshot_config};
use libfuzzer_sys::fuzz_target;

const FIXED_CONFIG: BookConfig = BookConfig {
    max_orders: 64,
    grid_len: 640,
    tick_size: 1,
    grid_start_tick: 1,
    default_stp: StpPolicy::CancelIncoming,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(state) = decode_snapshot(data, &FIXED_CONFIG) {
        let book = Book::from_snapshot_state(state)
            .expect("decoded snapshot state must satisfy Book::from_snapshot_state");
        let _ = book.best_bid();
        let _ = book.best_ask();
    }

    if let Ok(cfg) = snapshot_config(data)
        && cfg.max_orders <= 2_048
        && cfg.grid_len <= 16_384
        && let Ok(state) = decode_snapshot(data, &cfg)
    {
        let book = Book::from_snapshot_state(state)
            .expect("decoded snapshot state must satisfy Book::from_snapshot_state");
        let _ = book.live_order_count();
    }
});
