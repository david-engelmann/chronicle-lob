.PHONY: ci fmt lint fuzz-check test doc bench deny python-test examples book clean

ci: fmt lint fuzz-check test doc bench deny python-test examples

fmt:
	cargo fmt --all -- --check
	cargo fmt --manifest-path fuzz/Cargo.toml --all -- --check

lint:
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

fuzz-check:
	CARGO_TARGET_DIR=$(CURDIR)/target cargo check --manifest-path fuzz/Cargo.toml --locked --bins

test:
	cargo test --workspace --locked

doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

bench:
	cargo bench --workspace --locked -- --test

deny:
	cargo deny check

python-test:
	cargo build --locked -p chronicle-lob-cli --bin chronicle-cli
	python3 -m unittest discover -s python/tests -v

examples:
	cargo run --locked -p chronicle-core --example book_session > /tmp/book_session.out
	diff -u crates/chronicle-core/examples/book_session.expected.txt /tmp/book_session.out
	cargo run --locked -p chronicle-wal --example wal_recovery > /tmp/wal_recovery.out
	diff -u crates/chronicle-wal/examples/wal_recovery.expected.txt /tmp/wal_recovery.out
	cargo build --locked -p chronicle-lob-cli --bin chronicle-cli
	python3 examples/market_session_and_replay.py
	sh examples/crash_recovery_drill.sh

book:
	bash book/sync-docs.sh
	mdbook build book
	rm -rf book/src/docs

clean:
	cargo clean
	rm -rf book/build book/src/docs
