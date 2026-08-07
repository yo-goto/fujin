# fujin の開発タスク。
#
# .cargo/config.toml で既定ターゲットを wasm32-wasip1 にしているため、
# 素の `cargo test` は wasm 向けにテストをビルドしてしまい実行できない
# （wasm ランタイムが要る）。テストはホストターゲットで走らせる。

HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')

# インストール先。zellij の config ディレクトリは OS を問わず ~/.config/zellij に
# 解決されるので、その配下を既定にしておく（zellij 側にプラグインの置き場所の規約は無い）
PLUGIN_DIR ?= $(HOME)/.config/zellij/plugins

.PHONY: all build release install setup test fmt fmt-check lint check clean changelog

all: check

build:
	cargo build

release:
	cargo build --release

# 稼働中のセッションには反映されない。既存インスタンスは古い wasm のまま動くので、
# 入れ替えたらセッションを作り直すこと（start-or-reload-plugin は1インスタンスしか
# リロードしない）
install: release
	mkdir -p $(PLUGIN_DIR)
	cp target/wasm32-wasip1/release/fujin.wasm $(PLUGIN_DIR)/fujin.wasm

# 配置のあとのセットアップ（レイアウト生成 + Claude Code フック登録）。
# config.kdl は書き換えず、貼り付ける KDL 片を表示するだけ。
# 追加の引数は SETUP_ARGS で渡す（例: make setup SETUP_ARGS="--no-hooks --dry-run"）
setup: install
	./extras/setup.sh --plugin-dir $(PLUGIN_DIR) $(SETUP_ARGS)

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

# Conventional Commits に沿ったコミットから CHANGELOG.md を再生成する
# （git-cliff が必要: brew install git-cliff）。生成後は内容を確認してコミットすること
changelog:
	git-cliff -o CHANGELOG.md
