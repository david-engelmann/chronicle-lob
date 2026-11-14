//! The WAL writer (ARCHITECTURE.md section 9.4). Owns its sink and
//! one reused encode buffer sized to the largest frame, and applies
//! the configured sync policy per frame. The sink is a trait so tests
//! can count sync calls without a filesystem (INV-WAL-6).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use crate::error::WalError;
use crate::frame::{WalCommand, encode_frame};

/// When the writer asks the operating system to make frames durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Sync after every frame. The default, and the only policy the
    /// durability claim holds under.
    Always,
    /// Sync only when the caller takes a snapshot or closes.
    OnSnapshot,
    /// Never sync. For tests and benchmarks only.
    Never,
}

/// A byte sink that can be synced to durable storage. Implemented by
/// [`File`] for real logs and by test doubles that count syncs.
pub trait WalSink: Write {
    /// Flush this sink's bytes to durable storage.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error.
    fn sync_data(&mut self) -> io::Result<()>;
}

impl WalSink for File {
    fn sync_data(&mut self) -> io::Result<()> {
        Self::sync_data(self)
    }
}

/// Appends frames to one log, in sequence order. The writer does not
/// track sequence numbers itself: the caller passes the sequence the
/// book assigned, which is the frame's authoritative seq.
pub struct WalWriter<S: WalSink> {
    sink: S,
    buffer: Vec<u8>,
    policy: SyncPolicy,
}

impl WalWriter<File> {
    /// Open a log file for appending, creating it when missing.
    /// Existing content is kept: reopening a log to continue it is
    /// the normal crash recovery flow.
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] when the file cannot be opened.
    pub fn open(path: &Path, policy: SyncPolicy) -> Result<Self, WalError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| WalError::io("open WAL file", e))?;
        Ok(Self::new(file, policy))
    }
}

impl<S: WalSink> WalWriter<S> {
    /// Wrap a sink. The encode buffer starts empty and warms to the
    /// largest frame on first use.
    pub const fn new(sink: S, policy: SyncPolicy) -> Self {
        Self {
            sink,
            buffer: Vec::new(),
            policy,
        }
    }

    /// Encode and append one frame at sequence `seq`, then sync when
    /// the policy is [`SyncPolicy::Always`].
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] when the write or the sync fails.
    pub fn append(&mut self, command: &WalCommand, seq: u64) -> Result<(), WalError> {
        encode_frame(command, seq, &mut self.buffer);
        self.sink
            .write_all(&self.buffer)
            .map_err(|e| WalError::io("write WAL frame", e))?;
        if self.policy == SyncPolicy::Always {
            self.sync_now()?;
        }
        Ok(())
    }

    /// Sync the sink now. Snapshot save calls this under
    /// [`SyncPolicy::OnSnapshot`]; close paths call it under every
    /// policy except [`SyncPolicy::Never`].
    ///
    /// # Errors
    ///
    /// Returns [`WalError::Io`] when the sync fails.
    pub fn sync_now(&mut self) -> Result<(), WalError> {
        self.sink
            .sync_data()
            .map_err(|e| WalError::io("sync WAL file", e))
    }

    /// Capacity of the reused encode buffer, exposed so tests can
    /// prove it stops growing (INV-ALLOC-3).
    #[must_use]
    pub const fn buffer_capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// The sink, for inspection in tests.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Unwrap the writer and return the sink.
    pub fn into_inner(self) -> S {
        self.sink
    }
}

#[cfg(test)]
mod tests {
    use chronicle_core::{OrderId, ParticipantId, Side};

    use super::*;
    use crate::frame::encode_frame;

    /// A sink that records its bytes and counts its syncs.
    #[derive(Default)]
    struct CountingSink {
        bytes: Vec<u8>,
        syncs: usize,
    }

    impl Write for CountingSink {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl WalSink for CountingSink {
        fn sync_data(&mut self) -> io::Result<()> {
            self.syncs += 1;
            Ok(())
        }
    }

    fn add(seq: u64) -> WalCommand {
        WalCommand::Add {
            order_id: OrderId(seq),
            participant: ParticipantId(1),
            side: Side::Bid,
            order_type: chronicle_core::OrderType::Limit,
            stp: None,
            price: 100,
            qty: 1,
        }
    }

    #[test]
    fn wal_writer_sync_policy_always_calls_sync() {
        // INV-WAL-6: one sync per append under Always, none under
        // Never, none under OnSnapshot until asked.
        let mut always = WalWriter::new(CountingSink::default(), SyncPolicy::Always);
        for seq in 1..=3 {
            always.append(&add(seq), seq).expect("append");
        }
        assert_eq!(always.sink().syncs, 3);

        let mut never = WalWriter::new(CountingSink::default(), SyncPolicy::Never);
        for seq in 1..=3 {
            never.append(&add(seq), seq).expect("append");
        }
        assert_eq!(never.sink().syncs, 0);

        let mut on_snapshot = WalWriter::new(CountingSink::default(), SyncPolicy::OnSnapshot);
        for seq in 1..=3 {
            on_snapshot.append(&add(seq), seq).expect("append");
        }
        assert_eq!(on_snapshot.sink().syncs, 0);
        on_snapshot.sync_now().expect("sync");
        assert_eq!(on_snapshot.sink().syncs, 1);

        // The bytes on the sink are exactly the encoded frames.
        let mut expected = Vec::new();
        let mut frame = Vec::new();
        for seq in 1..=3 {
            encode_frame(&add(seq), seq, &mut frame);
            expected.extend_from_slice(&frame);
        }
        assert_eq!(always.sink().bytes, expected);
    }

    #[test]
    fn wal_append_reuses_buffer() {
        // INV-ALLOC-3 (Milestone 3 half): after the buffer warms to
        // the largest frame, 10,000 appends never grow it.
        let mut writer = WalWriter::new(CountingSink::default(), SyncPolicy::Never);
        writer
            .append(&add(1), 1)
            .expect("warm up with the largest kind");
        let warmed = writer.buffer_capacity();
        assert!(warmed >= 72, "an add frame is 72 bytes");
        for seq in 2..=10_000u64 {
            writer.append(&add(seq), seq).expect("append");
        }
        assert_eq!(writer.buffer_capacity(), warmed);
        assert_eq!(writer.sink().bytes.len(), 72 * 10_000);
    }
}
