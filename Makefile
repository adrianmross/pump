.PHONY: fmt lint test build ci demo dist-plan dist-build

fmt:
	cargo fmt --check

lint:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

build:
	cargo build --release --locked

ci: fmt lint test build

demo:
	scripts/generate-demo-gif.sh

dist-plan:
	dist plan --allow-dirty

dist-build:
	dist build --allow-dirty
