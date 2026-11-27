#![no_main]

use std::io::Cursor;

use chronicle_core::{BookConfig, StpPolicy};
use chronicle_wal::{recover, recover_from_reader, scan_log, scan_reader_with};
use libfuzzer_sys::fuzz_target;

const FUZZ_CONFIG: BookConfig = BookConfig {
    max_orders: 64,
    grid_len: 640,
    tick_size: 1,
    grid_start_tick: 1,
    default_stp: StpPolicy::CancelIncoming,
};

fuzz_target!(|data: &[u8]| {
    let slice_scan = scan_log(data);
    let mut streamed_seqs = Vec::new();
    let stream_scan = scan_reader_with(Cursor::new(data), |frame| {
        streamed_seqs.push(frame.seq);
        Ok(())
    });

    match (&slice_scan, &stream_scan) {
        (Ok(slice_out), Ok(stream_rep)) => {
            assert_eq!(slice_out.frames.len() as u64, stream_rep.frame_count);
            assert_eq!(
                slice_out.discarded_trailing_bytes,
                stream_rep.discarded_trailing_bytes
            );
            assert_eq!(slice_out.frames.last().map(|f| f.seq), stream_rep.last_seq);
        }
        (Err(_), Err(_)) => {}
        (slice_res, stream_res) => {
            panic!("slice scan {slice_res:?} disagreed with streaming scan {stream_res:?}");
        }
    }

    let slice_rec = recover(FUZZ_CONFIG, None, data);
    let stream_rec = recover_from_reader(FUZZ_CONFIG, None, Cursor::new(data));
    if let (Ok(a), Ok(b)) = (slice_rec, stream_rec) {
        assert_eq!(a.report, b.report);
        assert_eq!(a.book.live_order_count(), b.book.live_order_count());
        assert_eq!(a.book.best_bid(), b.book.best_bid());
        assert_eq!(a.book.best_ask(), b.book.best_ask());
    }
});
