#!/usr/bin/env bash
# fujin セットアップ補助スクリプト
#
# README の手順のうち、手作業で踏みやすい／退屈な2箇所を機械化する:
#
#   1. レイアウトファイルの生成
#      `children` と `pane` の取り違え（タブのターミナルが0個になり zellij が終了する）
#      と、`new_tab_template` の書き漏らし（セッションマネージャ経由の起動で
#      フォールバックが効かない）を、生成物の側で起こらないようにする。
#
#   2. Claude Code フックの登録
#      同じパスを10イベントへ書き写すだけの作業。フック本体は wasm と同じ場所へ
#      コピーしてからそのパスを登録するので、このリポジトリを移動・削除しても
#      登録が壊れない。
#
# config.kdl は編集しない。`plugins {}` / `keybinds {}` は既存ブロックへの
# マージが要る（`clear-defaults` やモード別の入れ子もある）ので、テキスト処理で
# 壊したときの復旧が重い。貼り付ける KDL 片を出力するに留める。

set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
hook_src="$script_dir/claude-hooks/fujin-hook.sh"

# ---------------------------------------------------------------- defaults

repo=yo-goto/fujin

config_dir=${ZELLIJ_CONFIG_DIR:-$HOME/.config/zellij}
plugin_dir=
layout_name=fujin
sidebar_width=32
claude_settings=$HOME/.claude/settings.json
release_tag=latest
do_download=0
do_layout=1
do_hooks=1
do_config=1
force=0
dry_run=0

usage() {
  cat <<'EOS'
usage: setup.sh [options]

Sets fujin up: optionally downloads the plugin from GitHub releases, generates
the zellij layout for the sidebar, and registers the Claude Code hooks.
Never edits config.kdl -- it prints the snippet to paste instead.

  -d, --download        download fujin.wasm and the hook from a GitHub release
                        (needed when running this script on its own; without it,
                        the wasm has to be in place already, e.g. via `make install`)
      --version TAG     release tag to download (default: latest)

  --config-dir DIR      zellij config dir (default: $ZELLIJ_CONFIG_DIR or ~/.config/zellij)
  --plugin-dir DIR      where fujin.wasm lives (default: <config-dir>/plugins)
  --layout-name NAME    layout file basename (default: fujin)
  --width N             sidebar width in columns (default: 32)
  --claude-settings F   Claude Code settings.json (default: ~/.claude/settings.json)

  --layout-only         only generate the layout file
  --hooks-only          only register the Claude Code hooks
  --config-only         only print the config.kdl snippet
  --no-hooks            skip the Claude Code hooks

  -f, --force           overwrite an existing layout file without asking
  -n, --dry-run         show what would happen, write nothing
  -h, --help            this message
EOS
}

while [ $# -gt 0 ]; do
  case "$1" in
    -d|--download) do_download=1; shift ;;
    --version) release_tag=$2; shift 2 ;;
    --config-dir) config_dir=$2; shift 2 ;;
    --plugin-dir) plugin_dir=$2; shift 2 ;;
    --layout-name) layout_name=$2; shift 2 ;;
    --width) sidebar_width=$2; shift 2 ;;
    --claude-settings) claude_settings=$2; shift 2 ;;
    --layout-only) do_hooks=0; do_config=0; shift ;;
    --hooks-only) do_layout=0; do_config=0; shift ;;
    --config-only) do_layout=0; do_hooks=0; shift ;;
    --no-hooks) do_hooks=0; shift ;;
    -f|--force) force=1; shift ;;
    -n|--dry-run) dry_run=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'setup.sh: unknown option: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ "$release_tag" = latest ]; then
  release_base="https://github.com/$repo/releases/latest/download"
else
  release_base="https://github.com/$repo/releases/download/$release_tag"
fi

: "${plugin_dir:=$config_dir/plugins}"

layout_file="$config_dir/layouts/$layout_name.kdl"
wasm_path="$plugin_dir/fujin.wasm"
hook_dst="$plugin_dir/fujin-hook.sh"

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
die()  { printf 'setup.sh: %s\n' "$*" >&2; exit 1; }

# `~` 付きで書ける場所なら `~` に畳む。zellij は shellexpand で展開するので、
# ホームディレクトリ名が設定に残らない（docs/issues/config-and-distribution.md §1.3）
tildify() {
  case "$1" in
    "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;;
    *) printf '%s' "$1" ;;
  esac
}

confirm() {
  [ -t 0 ] || return 1
  printf '    %s [y/N] ' "$1"
  local reply
  read -r reply || return 1
  case "$reply" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

wasm_location="file:$(tildify "$wasm_path")"

# ---------------------------------------------------------------- download

# リリースから wasm とフック本体を取ってくる。リポジトリを clone せず
# setup.sh 単体を落として実行する経路のためにある。
# 承認結果は展開後の絶対パス単位で記録されるので、同じ場所へ上書きする限り
# 更新しても再承認にはならない（permissions.kdl のキーを実測して確認）
download_assets() {
  step "Download  $release_base"

  command -v curl >/dev/null 2>&1 || die "curl is required for --download"

  if [ "$dry_run" -eq 1 ]; then
    info "would download fujin.wasm -> $wasm_path"
    info "would download fujin-hook.sh -> $hook_dst"
    return 0
  fi

  mkdir -p "$plugin_dir"

  # 落としきってから差し替える。途中で失敗しても、動いている wasm を壊さない
  local tmp="$wasm_path.download.$$"
  curl -fsSL "$release_base/fujin.wasm" -o "$tmp" \
    || { rm -f "$tmp"; die "failed to download fujin.wasm from $release_base"; }
  mv "$tmp" "$wasm_path"
  ok "downloaded $wasm_path"

  if [ "$do_hooks" -eq 1 ]; then
    tmp="$hook_dst.download.$$"
    curl -fsSL "$release_base/fujin-hook.sh" -o "$tmp" \
      || { rm -f "$tmp"; die "failed to download fujin-hook.sh from $release_base"; }
    mv "$tmp" "$hook_dst"
    chmod +x "$hook_dst"
    ok "downloaded $hook_dst"
    # 以降のコピー段はもう要らない
    hook_src=$hook_dst
  fi
}

# ---------------------------------------------------------------- layout

layout_body() {
  cat <<EOS
// fujin — resident sidebar layout (generated by extras/setup.sh)
//
// The same content is written into both default_tab_template and
// new_tab_template on purpose: with only default_tab_template, the fallback
// does not kick in for sessions created through the session manager
// (Ctrl+o -> w) with a chosen layout.
//
// The inner pane is \`pane\`, not \`children\`. In a template-only layout with no
// \`tab\` node there is nothing for \`children\` to insert, so you would get a tab
// with zero terminal panes -- which makes zellij exit at session creation.

layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=$sidebar_width borderless=true {
                plugin location="$layout_name"
            }
            pane
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }

    new_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=$sidebar_width borderless=true {
                plugin location="$layout_name"
            }
            pane
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }
}
EOS
}

install_layout() {
  step "Layout  $layout_file"

  if [ -e "$layout_file" ] && [ "$force" -eq 0 ]; then
    if [ "$dry_run" -eq 1 ]; then
      info "exists; would ask before overwriting (use --force to overwrite)"
      return 0
    fi
    if ! confirm "$layout_file already exists. Overwrite?"; then
      todo "kept the existing file (nothing written)"
      return 0
    fi
  fi

  if [ "$dry_run" -eq 1 ]; then
    info "would write:"
    layout_body | sed 's/^/    | /'
    return 0
  fi

  mkdir -p "$(dirname "$layout_file")"
  layout_body >"$layout_file"
  ok "wrote the layout (sidebar width $sidebar_width, plugin alias \"$layout_name\")"
}

# ---------------------------------------------------------------- hooks

# 10イベント。Notification だけは種別を絞るための matcher を付ける
hook_events="SessionStart UserPromptSubmit Stop StopFailure SessionEnd \
SubagentStart SubagentStop TaskCreated TaskCompleted"
hook_matcher='permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog'

install_hooks() {
  step "Claude Code hooks  $claude_settings"

  command -v jq >/dev/null 2>&1 || die "jq is required for hook registration (brew install jq)"
  if [ ! -f "$hook_src" ]; then
    die "hook script not found: $hook_src
    (running setup.sh outside the repository? re-run with --download)"
  fi

  # フック本体を wasm と同じ場所へ置く。settings.json に残るパスがこのリポジトリを
  # 指さなくなるので、リポジトリを移動・削除しても登録が壊れない
  if [ "$hook_src" = "$hook_dst" ]; then
    : # --download が既にそこへ置いている
  elif [ "$dry_run" -eq 1 ]; then
    info "would copy $hook_src -> $hook_dst"
  else
    mkdir -p "$(dirname "$hook_dst")"
    cp "$hook_src" "$hook_dst"
    chmod +x "$hook_dst"
    ok "installed the hook script at $hook_dst"
  fi

  local current='{}'
  if [ -s "$claude_settings" ]; then
    current=$(cat "$claude_settings")
    printf '%s' "$current" | jq -e . >/dev/null 2>&1 \
      || die "$claude_settings is not valid JSON; fix it first"
  fi

  # 既存の fujin-hook.sh 登録はすべて新しいパスへ書き換えてから追加する。
  # 二重登録にならず、リポジトリを動かしたあとの再実行が「直す」操作になる
  local updated
  updated=$(printf '%s' "$current" | jq \
    --arg cmd "$hook_dst" \
    --arg matcher "$hook_matcher" \
    --args '
      def has_cmd($ev): [.hooks[$ev][]?.hooks[]?.command] | index($cmd) != null;
      def entry: { type: "command", command: $cmd };

      (.hooks //= {})
      | .hooks |= walk(
          if type == "object" and .type == "command"
             and (.command | type == "string")
             and (.command | test("fujin-hook\\.sh$"))
          then .command = $cmd else . end
        )
      | reduce $ARGS.positional[] as $ev (.;
          .hooks[$ev] //= []
          | if has_cmd($ev) then . else .hooks[$ev] += [{ hooks: [entry] }] end
        )
      | .hooks["Notification"] //= []
      | if has_cmd("Notification") then .
        else .hooks["Notification"] += [{ matcher: $matcher, hooks: [entry] }] end
    ' $hook_events)

  if [ -s "$claude_settings" ] &&
     [ "$(printf '%s' "$current" | jq -S .)" = "$(printf '%s' "$updated" | jq -S .)" ]; then
    ok "hooks already registered for all 10 events (no change)"
    return 0
  fi

  if [ "$dry_run" -eq 1 ]; then
    info "would write $claude_settings:"
    printf '%s\n' "$updated" | sed 's/^/    | /'
    return 0
  fi

  if [ -s "$claude_settings" ]; then
    local backup="$claude_settings.fujin-backup.$(date +%Y%m%d%H%M%S)"
    cp "$claude_settings" "$backup"
    info "backed up to $backup"
  fi
  mkdir -p "$(dirname "$claude_settings")"
  printf '%s\n' "$updated" >"$claude_settings"
  ok "registered the hook for 10 events (9 plain + Notification with a matcher)"
  info "hooks take effect in newly started Claude Code sessions"
}

# ---------------------------------------------------------------- config.kdl

config_snippet() {
  cat <<EOS
plugins {
    $layout_name location="$wasm_location"
}

// if you already have a keybinds block, add just the bind inside it
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

print_config() {
  step "config.kdl  $config_dir/config.kdl"

  local cfg="$config_dir/config.kdl"
  if [ -f "$cfg" ]; then
    if grep -qE "^[[:space:]]*$layout_name[[:space:]]+location=" "$cfg"; then
      ok "plugin alias \"$layout_name\" is defined"
    else
      todo "plugin alias \"$layout_name\" is not defined"
    fi
    if grep -q 'fujin_mode' "$cfg"; then
      ok "a keybinding sends fujin_mode"
    else
      todo "no keybinding sends fujin_mode (nav mode has no entry key)"
    fi
    if grep -qE "^[[:space:]]*default_layout[[:space:]]+\"$layout_name\"" "$cfg"; then
      ok "default_layout is \"$layout_name\""
    else
      todo "default_layout is not \"$layout_name\" (the sidebar won't be resident)"
    fi
  else
    warn "$cfg does not exist yet"
  fi

  printf '\n    This script does not edit config.kdl. Add the missing pieces by hand:\n\n'
  config_snippet | sed 's/^/    /'
  printf '\n'
  info "\"Ctrl y\" is free in zellij's defaults; change it if it is taken."
  info "Keep 'floating true' -- without it, a session with no fujin opens it tiled."
  info "Do not bind 'Alt Enter': it collides with Claude Code's Shift+Enter."
}

# ---------------------------------------------------------------- run

if [ "$dry_run" -eq 1 ]; then
  printf 'fujin setup (dry run -- nothing will be written)\n'
else
  printf 'fujin setup\n'
fi

[ "$do_download" -eq 1 ] && download_assets
[ "$do_layout" -eq 1 ] && install_layout
[ "$do_hooks" -eq 1 ] && install_hooks
[ "$do_config" -eq 1 ] && print_config

if [ "$do_layout" -eq 1 ] || [ "$do_config" -eq 1 ]; then
  step "Remaining manual steps"
  if [ -f "$wasm_path" ]; then
    ok "$wasm_path is in place"
  else
    todo "$wasm_path is missing -- re-run with --download, or 'make install' from a clone"
  fi
  todo "approve the permission prompt on first load (focus the sidebar, press y)"
fi

printf '\n'
