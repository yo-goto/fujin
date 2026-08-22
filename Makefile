# fujin の開発タスク。
#
# .cargo/config.toml で既定ターゲットを wasm32-wasip1 にしているため、
# 素の `cargo test` は wasm 向けにテストをビルドしてしまい実行できない
# （wasm ランタイムが要る）。テストはホストターゲットで走らせる。
#
# 依存解決を伴う cargo 呼び出しには --locked を付けている。crates.io は同一
# バージョンの再公開ができず Cargo.lock は sha256 を持つので、lock を固定して
# いる限り「既に取り込んだ版が後から差し替わる」経路は塞がる。--locked が無いと
# ビルドついでに黙って解決し直され、その保証が外れる。
# lock が古いと落ちるので、依存を変えたときは意図して `cargo update` を実行する。

HOST_TARGET := $(shell rustc -vV | sed -n 's/^host: //p')

# インストール先。zellij の config ディレクトリは OS を問わず ~/.config/zellij に
# 解決されるので、その配下を既定にしておく（zellij 側にプラグインの置き場所の規約は無い）
PLUGIN_DIR ?= $(HOME)/.config/zellij/plugins

.PHONY: all build release install setup try test fmt fmt-check lint check check-snapshots clean changelog readme

all: check

build:
	cargo build --locked

release:
	cargo build --release --locked

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

# 別 worktree のビルドを、常駐セッションに一切触らずに専用セッションで試す。
# 専用の config dir を作り、その worktree の target を直接読ませる
# （例: make try WORKTREE=second、追加の引数は TRY_ARGS）
try:
	./extras/try-worktree.sh $(WORKTREE) $(TRY_ARGS)

test:
	cargo test --locked --target $(HOST_TARGET)

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# wasm 向け（本番のビルド構成）とホスト向け（テストコードを含む）の両方を見る
lint:
	cargo clippy --locked --all-targets -- -D warnings
	cargo clippy --locked --target $(HOST_TARGET) --all-targets -- -D warnings

# golden file の後始末を検査する（.docs/issues/issue-ui-requirements-approach.md 層2）。
#
# - 未承認: `.snap.new` を放置するとテストは落ち続ける。`cargo insta review` か
#   `INSTA_UPDATE=always make test` で畳んでからコミットする
# - 孤児: テストを消したのに `.snap` が残ると、絵だけが残って実装と対応しなくなる。
#   ファイル名（`fujin__<モジュール>__<名前>.snap`）からモジュールのファイルを導出し、
#   **そのファイルの中だけ**で `fn <名前>` か文字列 `"<名前>"`（名前付き
#   `assert_snapshot!` 用）を照合する。src/ 全体を見ると、別ファイルの同名テストが
#   孤児を隠す。同一テスト内の複数アサーションで付く `-2` 等の連番も落として照合する
#   （`cargo insta test --unreferenced` は cargo-insta CLI が要るので使わない）
check-snapshots:
	@snapdir=src/tests/snapshots; \
	if [ -d $$snapdir ]; then \
	  pending=$$(find $$snapdir -name '*.snap.new'); \
	  if [ -n "$$pending" ]; then \
	    echo "未承認の golden file がある:"; echo "$$pending"; \
	    echo "cargo insta review か INSTA_UPDATE=always make test で畳むこと"; \
	    exit 1; \
	  fi; \
	  orphan=""; \
	  for snap in $$(find $$snapdir -name '*.snap'); do \
	    base=$$(basename $$snap .snap); \
	    name=$${base##*__}; \
	    mod=$${base#fujin__}; mod=$$(echo "$${mod%__*}" | sed 's/__/\//g'); \
	    names=$$name; \
	    trimmed=$$(echo "$$name" | sed 's/-[0-9][0-9]*$$//'); \
	    [ "$$trimmed" != "$$name" ] && names="$$names $$trimmed"; \
	    files=""; \
	    [ -f "src/$$mod.rs" ] && files="src/$$mod.rs"; \
	    [ -f "src/$$mod/mod.rs" ] && files="$$files src/$$mod/mod.rs"; \
	    [ -n "$$files" ] || files=$$(find src -name '*.rs'); \
	    found=""; \
	    for n in $$names; do \
	      for f in $$files; do \
	        if grep -Fq "fn $$n(" "$$f" || grep -Fq "\"$$n\"" "$$f"; then found=1; fi; \
	      done; \
	    done; \
	    [ -n "$$found" ] || orphan="$$orphan $$snap"; \
	  done; \
	  if [ -n "$$orphan" ]; then \
	    echo "対応するテストが無い golden file がある:$$orphan"; exit 1; \
	  fi; \
	fi

check: fmt-check lint test check-snapshots

clean:
	cargo clean

# Conventional Commits に沿ったコミットから CHANGELOG.md を再生成する
# （git-cliff が必要。flake.nix の devShell に入っていれば揃っている。
# 素の環境なら brew install git-cliff）。生成後は内容を確認してコミットすること
changelog:
	git-cliff -o CHANGELOG.md

# README の設定節を src/config.rs の SETTINGS から再生成する（決定40）。
# 生成物とのずれは make test 側で落ちるので、落ちたらこれを実行する
readme:
	UPDATE_README=1 cargo test --locked --target $(HOST_TARGET) readme_settings_section
