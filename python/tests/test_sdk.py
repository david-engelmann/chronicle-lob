from __future__ import annotations

import importlib.util
from pathlib import Path
import struct
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
if importlib.util.find_spec("chronicle_lob") is None:
    sys.path.insert(0, str(ROOT / "python"))

import chronicle_lob  # noqa: E402


def _repack_frame_crc(frame: bytearray) -> bytes:
    header_prefix = bytes(frame[:28])
    payload = bytes(frame[32:])
    crc = chronicle_lob._frame_crc(header_prefix, payload)
    frame[28:32] = struct.pack("<I", crc)
    return bytes(frame)


class ChronicleLobSdkTests(unittest.TestCase):
    def test_version_matches_package(self) -> None:
        self.assertEqual(chronicle_lob.__version__, "0.3.3")
        self.assertEqual(chronicle_lob.version(), chronicle_lob.__version__)

    def test_completions_all_shells_and_invalid_shell(self) -> None:
        default_script = chronicle_lob.completions()
        self.assertIn("chronicle-cli", default_script)
        for shell in ("bash", "zsh", "fish", "powershell", "pwsh"):
            script = chronicle_lob.completions(shell)
            self.assertIn("chronicle-cli", script)
            for subcmd in (
                "replay",
                "verify",
                "snapshot",
                "checkpoint",
                "bench",
                "completions",
            ):
                self.assertIn(subcmd, script)
        self.assertEqual(
            chronicle_lob.completions("powershell"),
            chronicle_lob.completions("pwsh"),
        )
        with self.assertRaises(chronicle_lob.ChronicleError) as ctx:
            chronicle_lob.completions("tcsh")
        self.assertEqual(ctx.exception.exit_code, 2)

    def test_crc32c_golden_vector(self) -> None:
        self.assertEqual(chronicle_lob.crc32c(b"123456789"), 0xE3069283)
        self.assertEqual(chronicle_lob.crc32c(b""), 0)

    def test_bench_determinism(self) -> None:
        first = chronicle_lob.bench(ops=5_000, seed=42)
        second = chronicle_lob.bench(ops=5_000, seed=42)
        self.assertEqual(first, second)
        self.assertEqual(first.ops, 5_000)
        self.assertEqual(first.applied + first.rejected, 5_000)

    def test_wal_builder_verify_replay_and_checkpoint(self) -> None:
        config = chronicle_lob.BookSpec(
            max_orders=64,
            grid_len=640,
            tick_size=1,
            grid_start_tick=1,
        )
        builder = chronicle_lob.WalBuilder()
        builder.add(order_id=1, participant=10, side="bid", price=100, qty=5)
        builder.add(order_id=2, participant=20, side="ask", price=102, qty=4)
        builder.add(order_id=3, participant=30, side="bid", price=102, qty=3)

        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            wal_path = builder.write(tmp_dir / "session.wal")

            ver = chronicle_lob.verify(wal_path, config=config)
            self.assertTrue(ver.ok)
            self.assertEqual(ver.frames, 3)
            self.assertEqual(ver.last_seq, 3)

            rep = chronicle_lob.replay(wal_path, config=config, seed=7)
            self.assertEqual(rep.seed, 7)
            kinds = [ev.kind for ev in rep.events]
            self.assertEqual(kinds, ["accepted", "accepted", "trade", "filled"])
            self.assertEqual(rep.events[2].fields["price"], 102)
            self.assertEqual(rep.events[2].fields["qty"], 3)

            snap_path = tmp_dir / "book.snapshot"
            cp = chronicle_lob.checkpoint(wal_path, snap_path, config=config)
            self.assertEqual(cp.last_included_seq, 3)
            self.assertEqual(cp.live_orders, 2)
            self.assertTrue(cp.out_path.is_file())
            self.assertTrue(cp.archived_log.is_file())

            archived_frames = chronicle_lob.read_wal_frames(cp.archived_log)
            self.assertEqual(len(archived_frames), 3)
            header = chronicle_lob.read_snapshot_header(cp.out_path)
            self.assertEqual(header.last_included_seq, 3)
            self.assertEqual(header.next_seq, 4)
            self.assertEqual(header.live_order_count, 2)
            self.assertEqual(header.config, config)

    def test_wal_frames_roundtrip_all_command_types(self) -> None:
        self.assertEqual(chronicle_lob.read_wal_frames(b""), ())

        builder = chronicle_lob.WalBuilder()
        builder.add(order_id=1, participant=10, side="bid", price=100, qty=5)
        builder.add(
            order_id=2,
            participant=20,
            side="ask",
            price=105,
            qty=8,
            order_type="post-only",
            stp="cancel-both",
        )
        builder.add(
            order_id=3,
            participant=30,
            side="bid",
            price=105,
            qty=2,
            order_type="ioc",
            stp="cancel-incoming",
        )
        builder.add(
            order_id=4,
            participant=40,
            side="ask",
            price=100,
            qty=1,
            order_type="fok",
            stp="cancel-resting",
        )
        builder.amend(order_id=1, new_price=99, new_qty=7)
        builder.cancel(order_id=2)
        builder.cancel_by_participant(participant=10)

        raw = builder.to_bytes()
        frames = chronicle_lob.read_wal_frames(raw)
        self.assertEqual(frames, tuple(chronicle_lob.iter_wal_frames(raw)))
        self.assertEqual(len(frames), 7)

        f1, f2, f3, f4, f5, f6, f7 = frames
        self.assertEqual(f1.sequence, 1)
        self.assertEqual(f1.command_type, "add")
        self.assertEqual(f1.order_id, 1)
        self.assertEqual(f1.participant, 10)
        self.assertEqual(f1.side, "bid")
        self.assertEqual(f1.order_type, "limit")
        self.assertIsNone(f1.stp)
        self.assertEqual(f1.price, 100)
        self.assertEqual(f1.qty, 5)
        self.assertEqual(len(f1.payload), 40)

        self.assertEqual(
            (f2.sequence, f2.command_type, f2.side, f2.order_type, f2.stp),
            (2, "add", "ask", "post-only", "cancel-both"),
        )
        self.assertEqual(
            (f3.sequence, f3.order_type, f3.stp),
            (3, "ioc", "cancel-incoming"),
        )
        self.assertEqual(
            (f4.sequence, f4.order_type, f4.stp),
            (4, "fok", "cancel-resting"),
        )

        self.assertEqual(f5.sequence, 5)
        self.assertEqual(f5.command_type, "amend")
        self.assertEqual(f5.order_id, 1)
        self.assertIsNone(f5.participant)
        self.assertIsNone(f5.side)
        self.assertEqual(f5.price, 99)
        self.assertEqual(f5.qty, 7)
        self.assertEqual(len(f5.payload), 32)

        self.assertEqual(f6.sequence, 6)
        self.assertEqual(f6.command_type, "cancel")
        self.assertEqual(f6.order_id, 2)
        self.assertIsNone(f6.participant)
        self.assertIsNone(f6.price)
        self.assertEqual(len(f6.payload), 16)

        self.assertEqual(f7.sequence, 7)
        self.assertEqual(f7.command_type, "mass-cancel")
        self.assertIsNone(f7.order_id)
        self.assertEqual(f7.participant, 10)
        self.assertEqual(len(f7.payload), 16)

        with tempfile.TemporaryDirectory() as tmp:
            wal_path = builder.write(Path(tmp) / "all.wal")
            self.assertEqual(chronicle_lob.read_wal_frames(wal_path), frames)

        post_cp = chronicle_lob.WalBuilder(start_seq=10)
        post_cp.cancel(order_id=99)
        post_cp.cancel_by_participant(participant=3)
        decoded_post = chronicle_lob.read_wal_frames(post_cp.to_bytes())
        self.assertEqual([f.sequence for f in decoded_post], [10, 11])

    def test_wal_decoder_rejects_corruption_and_reports_offsets(self) -> None:
        builder = chronicle_lob.WalBuilder()
        builder.add(order_id=1, participant=1, side="bid", price=100, qty=10)
        builder.cancel(order_id=1)
        raw = builder.to_bytes()
        # Frame 1 is 72 bytes (32 header + 40 add payload); frame 2 starts at offset 72.
        self.assertEqual(len(raw), 72 + 48)

        # Bad magic at offset 0 and offset 72.
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 0.*bad magic"):
            chronicle_lob.read_wal_frames(b"BAD!" + raw[4:])
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 72.*bad magic"):
            chronicle_lob.read_wal_frames(raw[:72] + b"CSNP" + raw[76:])

        # Bad version at offset 72.
        bad_ver = bytearray(raw)
        struct.pack_into("<H", bad_ver, 72 + 4, 2)
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 72.*version"):
            chronicle_lob.read_wal_frames(bytes(bad_ver))

        # Zero sequence at offset 0.
        zero_seq = bytearray(raw[:72])
        struct.pack_into("<Q", zero_seq, 8, 0)
        with self.assertRaisesRegex(
            chronicle_lob.ChronicleError, r"offset 0.*zero sequence"
        ):
            chronicle_lob.read_wal_frames(_repack_frame_crc(zero_seq))

        # Sequence gap at offset 72 (seq 1 followed by seq 3 with valid CRC).
        gap_builder = chronicle_lob.WalBuilder()
        gap_builder.add(order_id=1, participant=1, side="bid", price=100, qty=10, seq=1)
        gap_builder.cancel(order_id=1, seq=3)
        with self.assertRaisesRegex(
            chronicle_lob.ChronicleError, r"offset 72.*expected 2, found 3"
        ):
            chronicle_lob.read_wal_frames(gap_builder.to_bytes())

        # Unknown command tag at offset 72.
        bad_kind = bytearray(raw)
        bad_kind[72 + 20] = 99
        with self.assertRaisesRegex(
            chronicle_lob.ChronicleError, r"offset 72.*unknown command tag 99"
        ):
            chronicle_lob.read_wal_frames(bytes(bad_kind))

        # Bit-flip in frame 2 payload -> CRC32C mismatch at offset 72.
        bit_flip = bytearray(raw)
        bit_flip[72 + 32] ^= 0x01
        with self.assertRaisesRegex(
            chronicle_lob.ChronicleError, r"offset 72.*CRC32C mismatch"
        ):
            chronicle_lob.read_wal_frames(bytes(bit_flip))

        # Truncated header and truncated payload at offset 0 and offset 72.
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 0"):
            chronicle_lob.read_wal_frames(raw[:12])
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 0"):
            chronicle_lob.read_wal_frames(raw[:50])
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 72"):
            chronicle_lob.read_wal_frames(raw[: 72 + 10])
        with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"offset 72"):
            chronicle_lob.read_wal_frames(raw[: 72 + 40])

        # Nonzero payload reserved byte with recomputed CRC rejected at offset 0.
        bad_reserved = bytearray(raw[:72])
        bad_reserved[32 + 15] = 1
        with self.assertRaisesRegex(
            chronicle_lob.ChronicleError, r"offset 0.*add\.reserved"
        ):
            chronicle_lob.read_wal_frames(_repack_frame_crc(bad_reserved))

    def test_snapshot_header_roundtrip_and_corruption(self) -> None:
        config = chronicle_lob.BookSpec(
            max_orders=64,
            grid_len=640,
            tick_size=5,
            grid_start_tick=10,
            default_stp="cancel-both",
        )
        builder = chronicle_lob.WalBuilder()
        builder.add(order_id=1, participant=10, side="bid", price=100, qty=5)
        builder.add(order_id=2, participant=20, side="ask", price=110, qty=4)
        builder.amend(order_id=1, new_price=105, new_qty=6)

        with tempfile.TemporaryDirectory() as tmp:
            tmp_dir = Path(tmp)
            wal_path = builder.write(tmp_dir / "session.wal")
            snap_path = tmp_dir / "session.snap"
            report = chronicle_lob.snapshot(wal_path, snap_path, config=config)
            self.assertEqual(report.last_included_seq, 3)
            self.assertEqual(report.live_orders, 2)

            snap_bytes = snap_path.read_bytes()
            header_from_path = chronicle_lob.read_snapshot_header(snap_path)
            header_from_bytes = chronicle_lob.read_snapshot_header(snap_bytes)
            self.assertEqual(header_from_path, header_from_bytes)
            self.assertEqual(header_from_path.version, 1)
            self.assertEqual(header_from_path.last_included_seq, 3)
            self.assertEqual(header_from_path.next_seq, 4)
            self.assertEqual(header_from_path.live_order_count, 2)
            self.assertEqual(header_from_path.config, config)
            expected_len = 68 + 8 * 64 + 20 * 640 + 56 * 2
            self.assertEqual(header_from_path.byte_len, expected_len)
            self.assertEqual(len(snap_bytes), expected_len)

            # Bit-flip inside body fails trailing snapshot CRC32C.
            body_flip = bytearray(snap_bytes)
            body_flip[80] ^= 0x04
            with self.assertRaisesRegex(
                chronicle_lob.ChronicleError, r"CRC32C mismatch"
            ):
                chronicle_lob.read_snapshot_header(bytes(body_flip))

            # Bit-flip in trailer fails trailing snapshot CRC32C.
            crc_flip = bytearray(snap_bytes)
            crc_flip[-1] ^= 0x80
            with self.assertRaisesRegex(
                chronicle_lob.ChronicleError, r"CRC32C mismatch"
            ):
                chronicle_lob.read_snapshot_header(bytes(crc_flip))

            # Truncation and bad magic fail cleanly.
            with self.assertRaises(chronicle_lob.ChronicleError):
                chronicle_lob.read_snapshot_header(b"")
            with self.assertRaises(chronicle_lob.ChronicleError):
                chronicle_lob.read_snapshot_header(snap_bytes[:40])
            with self.assertRaises(chronicle_lob.ChronicleError):
                chronicle_lob.read_snapshot_header(snap_bytes[:-1])
            with self.assertRaisesRegex(chronicle_lob.ChronicleError, r"bad magic"):
                chronicle_lob.read_snapshot_header(b"CWAL" + snap_bytes[4:])


if __name__ == "__main__":
    unittest.main()
