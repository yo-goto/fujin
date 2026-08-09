#!/usr/bin/env bash
# fujin worktree 試用スクリプト（開発者向け。利用者向けの導入は extras/setup.sh）
#
# 「今の常駐セッションを一切リロードせずに、別 worktree のビルドを試す」ためのもの。
# docs/issues/parallel-development-bottleneck.md の方針をそのまま実装している。
#
#   1. 対象 worktree を release ビルドする
#   2. その worktree 専用の zellij config dir を作る
#      （~/.config/zellij/ には一切触らない）
#   3. そこで専用セッションを起動する
#
# wasm は config dir へコピーせず、**worktree の target を直接指す**。
# 承認結果は展開後の wasm 絶対パス単位で記録される（config-and-distribution.md §2.2）
# ので、パスが worktree ごとに固定なら再ビルドしても承認が生き続ける。
#
# 常駐セッションと干渉しない理由:
#   - config dir が別なので config.kdl / layouts を書き換えない
#   - wasm パスが別なので zellij からは別プラグインに見える。
#     configuration 不一致で新規ペインが生える事故（redeploy-spawns-extra-plugin-pane.md）
#     も、同じセッションに同居しない以上そもそも起こらない
#   - cache dir は共通のままなので、権限の承認結果だけは共有される

set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
self_worktree=$(CDPATH= cd -- "$script_dir/.." && pwd -P)
# repos/<name>/extras/try-worktree.sh を想定。repos/ 直下を worktree の置き場所とみなす
repos_dir=$(dirname "$self_worktree")

# ---------------------------------------------------------------- defaults

worktree_arg=
session=
config_dir=
base_config=${ZELLIJ_CONFIG_DIR:-$HOME/.config/zellij}
layout_name=fujin
sidebar_width=32
minimal=0
do_build=1
do_grant=1
fresh=0
clean=0
nested=0
open_window=0
terminal=auto
dry_run=0

usage() {
  cat <<'EOS'
usage: try-worktree.sh [options] [<worktree>]

Builds a worktree and starts a throwaway zellij session that loads *that*
build, using a config dir of its own. Your everyday ~/.config/zellij and the
resident fujin session are never touched.

  <worktree>          directory name under repos/ (e.g. second), or a path
                      (default: the worktree this script lives in)

  -s, --session NAME  session name (default: fujin-try-<worktree>)
      --config-dir D  config dir to build (default: ~/.config/zellij-fujin-<worktree>)
      --base-config D config dir to copy as a starting point
                      (default: $ZELLIJ_CONFIG_DIR or ~/.config/zellij)
      --minimal       do not copy the base config; write a minimal config.kdl
      --width N       sidebar width in columns (default: 32)

      --no-build      skip `cargo build --release`
      --no-grant      do not pre-register the permissions (approve by hand instead)
      --fresh         kill an existing session of the same name first
      --open          open the session in a new terminal window and return
      --terminal CMD  terminal emulator for --open (default: autodetect)
      --nested        allow starting from inside a zellij session (nested)
      --clean         kill the session, delete the config dir, and exit

  -n, --dry-run       show what would happen, write nothing
  -h, --help          this message
EOS
}

while [ $# -gt 0 ]; do
  case "$1" in
    -s|--session) session=$2; shift 2 ;;
    --config-dir) config_dir=$2; shift 2 ;;
    --base-config) base_config=$2; shift 2 ;;
    --minimal) minimal=1; shift ;;
    --width) sidebar_width=$2; shift 2 ;;
    --layout-name) layout_name=$2; shift 2 ;;
    --no-build) do_build=0; shift ;;
    --no-grant) do_grant=0; shift ;;
    --fresh) fresh=1; shift ;;
    --open) open_window=1; shift ;;
    --terminal) terminal=$2; open_window=1; shift 2 ;;
    --nested) nested=1; shift ;;
    --clean) clean=1; shift ;;
    -n|--dry-run) dry_run=1; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) printf 'try-worktree.sh: unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
    *)
      [ -z "$worktree_arg" ] || { printf 'try-worktree.sh: too many arguments\n\n' >&2; usage >&2; exit 2; }
      worktree_arg=$1; shift ;;
  esac
done

# ---------------------------------------------------------------- helpers

if [ -t 1 ]; then
  c_bold=$'\033[1m'; c_green=$'\033[32m'; c_yellow=$'\033[33m'; c_off=$'\033[0m'
else
  c_bold=; c_green=; c_yellow=; c_off=
fi

step() { printf '\n%s==> %s%s\n' "$c_bold" "$*" "$c_off"; }
info() { printf '    %s\n' "$*"; }
ok()   { printf '    %sok%s   %s\n' "$c_green" "$c_off" "$*"; }
todo() { printf '    %stodo%s %s\n' "$c_yellow" "$c_off" "$*"; }
warn() { printf '    %swarning:%s %s\n' "$c_yellow" "$c_off" "$*" >&2; }
die()  { printf 'try-worktree.sh: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- resolve

# 引数はパスとしても worktree 名としても受ける。名前で来たら repos/ 配下を見る
if [ -z "$worktree_arg" ]; then
  worktree=$self_worktree
elif [ -d "$worktree_arg" ]; then
  worktree=$(CDPATH= cd -- "$worktree_arg" && pwd -P)
elif [ -d "$repos_dir/$worktree_arg" ]; then
  worktree=$(CDPATH= cd -- "$repos_dir/$worktree_arg" && pwd -P)
else
  die "no such worktree: $worktree_arg (looked in $repos_dir/)"
fi

[ -f "$worktree/Cargo.toml" ] || die "$worktree does not look like a fujin checkout (no Cargo.toml)"

name=$(basename "$worktree")
: "${session:=fujin-try-$name}"
: "${config_dir:=$HOME/.config/zellij-fujin-$name}"

wasm_path="$worktree/target/wasm32-wasip1/release/fujin.wasm"
layout_file="$config_dir/layouts/$layout_name.kdl"
config_file="$config_dir/config.kdl"

command -v zellij >/dev/null 2>&1 || die "zellij is not on PATH"

# ---------------------------------------------------------------- clean

kill_session() {
  # 存在しないセッションを消そうとしたときの終了コードは無視する
  zellij kill-session "$session" >/dev/null 2>&1 || true
  zellij delete-session "$session" >/dev/null 2>&1 || true
}

if [ "$clean" -eq 1 ]; then
  step "Clean  $session"
  if [ "$dry_run" -eq 1 ]; then
    info "would kill and delete the session $session"
    info "would remove $config_dir"
    exit 0
  fi
  kill_session
  ok "killed and deleted the session (if it existed)"
  # 消すのはこのスクリプトが作ったディレクトリだけ。ベース config dir と
  # 取り違えると普段の設定が消えるので、名前ではなく実体で照合する
  if [ "$config_dir" = "$base_config" ] || [ "$config_dir" = "$HOME/.config/zellij" ]; then
    die "refusing to remove $config_dir -- that is a real config dir"
  fi
  if [ -d "$config_dir" ]; then
    rm -rf "$config_dir"
    ok "removed $config_dir"
  else
    info "$config_dir does not exist"
  fi
  # 承認結果（cache dir 側）は残す。同じ worktree をまた試すときに効くため
  info "kept the permission grant for $wasm_path"
  printf '\n'
  exit 0
fi

printf 'fujin try-worktree\n'
info "worktree    $worktree"
info "wasm        $wasm_path"
info "config dir  $config_dir"
info "session     $session"

# ---------------------------------------------------------------- build

build() {
  step "Build  cargo build --release"
  if [ "$dry_run" -eq 1 ]; then
    info "would run: (cd $worktree && cargo build --release)"
    return 0
  fi
  (cd "$worktree" && cargo build --release)
  ok "built $wasm_path"
}

# ---------------------------------------------------------------- config dir

minimal_config() {
  cat <<EOS
// fujin — throwaway config for trying the "$name" worktree
// (generated by extras/try-worktree.sh; safe to delete)

plugins {
    $layout_name location="file:$wasm_path"
}

keybinds {
    shared_except "locked" {
        bind "Ctrl y" {
            MessagePlugin "$layout_name" {
                name "fujin_mode"
                floating true
            }
        }
    }
}

default_layout "$layout_name"
EOS
}

install_config() {
  step "Config dir  $config_dir"

  if [ "$dry_run" -eq 1 ]; then
    info "would recreate $config_dir"
    if [ "$minimal" -eq 0 ] && [ -f "$base_config/config.kdl" ]; then
      info "would copy $base_config (minus plugins/) and point the \"$layout_name\" alias at the worktree build"
    else
      info "would write a minimal config.kdl"
    fi
    return 0
  fi

  # 毎回作り直す。検証用なので中身を持ち越さないほうが事故が少ない
  rm -rf "$config_dir"
  mkdir -p "$config_dir/layouts"

  if [ "$minimal" -eq 0 ] && [ -f "$base_config/config.kdl" ]; then
    # plugins/ は wasm の置き場所（重いうえ、ここでは使わない）なので持ってこない。
    # 手元のバックアップ（*.bak）も、使い捨ての config dir には要らない
    (cd "$base_config" && find . -mindepth 1 -maxdepth 1 \
      ! -name plugins ! -name '*.bak' -exec cp -R {} "$config_dir/" \;)

    # エイリアスの location だけを worktree のビルドへ向け直す。
    # 1行の置換で済むのは、config.kdl 側が決定17でエイリアスに寄せてあるため
    if grep -qE "^[[:space:]]*$layout_name[[:space:]]+location=" "$config_file"; then
      local tmp="$config_file.try.$$"
      sed -E "s|^([[:space:]]*$layout_name[[:space:]]+location=\")[^\"]*(\")|\1file:$wasm_path\2|" \
        "$config_file" >"$tmp"
      mv "$tmp" "$config_file"
      ok "copied $base_config and pointed the \"$layout_name\" alias at this worktree"
    else
      minimal_config >"$config_file"
      warn "no \"$layout_name\" alias in $base_config/config.kdl -- wrote a minimal config instead"
      info "keybinds fall back to zellij's defaults; nav mode is on Ctrl+y"
    fi
  else
    minimal_config >"$config_file"
    ok "wrote a minimal config.kdl (nav mode on Ctrl+y)"
  fi

  # レイアウトは setup.sh に作らせる。`children` と `pane` の取り違えや
  # new_tab_template の書き漏らしを、生成物の側で防いでいるものをそのまま使う
  "$script_dir/setup.sh" --layout-only --force \
    --config-dir "$config_dir" \
    --plugin-dir "$(dirname "$wasm_path")" \
    --layout-name "$layout_name" \
    --width "$sidebar_width" >/dev/null
  ok "wrote $layout_file"
}

# ---------------------------------------------------------------- permissions

# 承認は「展開後の wasm 絶対パス」単位で cache dir の permissions.kdl に記録される。
# config dir を分けても cache dir は共通なので、ここへ先に書いておけば
# 初回の承認プロンプト（7種を手で承認する手間）を踏まずに済む。
# zellij は起動時にこのファイルを読み、実際に要求された権限だけを書き戻すので、
# 余分に書いても消えるだけで害はない（2026-08-09 実測）。
grant_permissions() {
  step "Permissions"

  local cache_dir perms
  cache_dir=$(zellij setup --check 2>/dev/null | sed -n 's/^\[CACHE DIR\]: *//p' | tr -d '"')
  [ -n "$cache_dir" ] || { warn "could not determine the cache dir; skipping"; return 0; }
  perms="$cache_dir/permissions.kdl"

  if [ -f "$perms" ] && grep -qF "\"$wasm_path\"" "$perms"; then
    ok "already granted for this worktree's build"
    return 0
  fi

  # 要求する権限は wasm 側の正本（src/main.rs）から拾う。手で並べると
  # 権限が増えたとき（決定42 の ReadPaneContents など）に取り残される
  local types
  types=$(grep -oE 'PermissionType::[A-Za-z]+' "$worktree/src/main.rs" 2>/dev/null \
    | sed 's/PermissionType:://' | sort -u)
  [ -n "$types" ] || { warn "could not read the permission list from src/main.rs; skipping"; return 0; }

  if [ "$dry_run" -eq 1 ]; then
    info "would append to $perms:"
    printf '    | "%s" {\n' "$wasm_path"
    printf '    |     %s\n' $types
    printf '    | }\n'
    return 0
  fi

  mkdir -p "$cache_dir"
  {
    printf '"%s" {\n' "$wasm_path"
    printf '    %s\n' $types
    printf '}\n'
  } >>"$perms"
  ok "pre-approved $(printf '%s\n' "$types" | wc -l | tr -d ' ') permissions for this worktree's build"
  info "delete the entry in $perms to get the prompt back"
}

# ---------------------------------------------------------------- run

# --open 用。指定が無ければ手元にあるものを探す
resolve_terminal() {
  local t
  if [ "$terminal" != auto ]; then
    command -v "$terminal" >/dev/null 2>&1 || return 1
    printf '%s' "$terminal"
    return 0
  fi
  for t in ${FUJIN_TERMINAL:-} ${TERMINAL:-} alacritty wezterm kitty ghostty; do
    if command -v "$t" >/dev/null 2>&1; then printf '%s' "$t"; return 0; fi
  done
  return 1
}

start_session() {
  step "Session  $session"

  local exists=0
  zellij list-sessions --no-formatting 2>/dev/null | awk '{print $1}' | grep -qx "$session" && exists=1

  if [ "$exists" -eq 1 ] && [ "$fresh" -eq 1 ]; then
    if [ "$dry_run" -eq 1 ]; then
      info "would kill the existing session first (--fresh)"
    else
      kill_session
      ok "killed the previous session (--fresh)"
      exists=0
    fi
  fi

  local -a cmd
  if [ "$exists" -eq 1 ]; then
    info "the session already exists -- attaching (use --fresh to recreate it)"
    cmd=(zellij attach "$session")
  else
    cmd=(zellij -s "$session")
  fi

  if [ "$dry_run" -eq 1 ]; then
    if [ "$open_window" -eq 1 ]; then
      info "would open a new $(resolve_terminal || printf '<terminal>') window running: ${cmd[*]}"
    else
      info "would run: ZELLIJ_CONFIG_DIR=$config_dir ${cmd[*]}"
    fi
    return 0
  fi

  local -a env_prefix
  env_prefix=(env -u ZELLIJ -u ZELLIJ_SESSION_NAME -u ZELLIJ_PANE_ID
    ZELLIJ_CONFIG_DIR="$config_dir")

  # 別ウィンドウで開く。zellij の中からでも使えるので、ネストの回避策にもなる
  if [ "$open_window" -eq 1 ]; then
    local term
    term=$(resolve_terminal) || die "no terminal emulator found (pass --terminal CMD)"
    # 「このコマンドを実行しろ」の渡し方はターミナルごとに違う。
    # wezterm だけサブコマンド形式で、残りは -e で揃う
    local -a exec_args
    case "$(basename "$term")" in
      wezterm) exec_args=(start --) ;;
      *) exec_args=(-e) ;;
    esac
    nohup "$term" "${exec_args[@]}" "${env_prefix[@]}" "${cmd[@]}" >/dev/null 2>&1 &
    disown
    ok "opened a new $term window running \"$session\""
    info "close it with: $0 --clean ${worktree_arg:-$name}"
    return 0
  fi

  # zellij の中から素直に起動するとネストを拒否される。ネストは操作しづらく
  # 事故りやすいので、既定では起動コマンドを出すだけにして、別のターミナル
  # ウィンドウで叩いてもらう
  if [ -n "${ZELLIJ:-}" ] && [ "$nested" -eq 0 ]; then
    todo "you are inside a zellij session -- run this in another terminal window:"
    printf '\n        ZELLIJ_CONFIG_DIR=%s %s\n\n' "$config_dir" "${cmd[*]}"
    info "or re-run with --open (new window) or --nested (here anyway)"
    return 0
  fi

  exec "${env_prefix[@]}" "${cmd[@]}"
}

[ "$do_build" -eq 1 ] && build
install_config
[ "$do_grant" -eq 1 ] && grant_permissions
start_session
