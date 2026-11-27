.PHONY: ci fmt lint test doc bench deny python-test clean

ci: fmt lint test doc bench deny python-test

fmt:
	cargo fmt --all -- --check

lint:
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

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

clean:
	cargo clean
