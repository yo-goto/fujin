# fujin の開発タスク。
#
# .cargo/config.toml で既定ターゲットを wasm32-wasip1 にしているため、
# 素の `cargo test` は wasm 向けにテストをビルドしてしまい実行できない
# （wasm ランタイムが要る）。テストはホストターゲットで走らせる。

HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')

.PHONY: all build release test fmt fmt-check lint check clean

all: check

build:
	cargo build

release:
	cargo build --release

test:
	cargo test --target $(HOST_TARGET)

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# wasm 向け（本番のビルド構成）とホスト向け（テストコードを含む）の両方を見る
lint:
	cargo clippy --all-targets -- -D warnings
	cargo clippy --target $(HOST_TARGET) --all-targets -- -D warnings

check: fmt-check lint test

clean:
	cargo clean
