#!/usr/bin/env bash
# fujin worktree 試用スクリプト（開発者向け。利用者向けの導入は extras/setup.sh）
#
# 「今の常駐セッションを一切リロードせずに、別 worktree のビルドを試す」ためのもの。
# docs/issues/issue-parallel-development-bottleneck.md の方針をそのまま実装している。
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
# --open した窓の中からこのスクリプトを呼び戻すので、相対パスの $0 では届かない
self_sh="$script_dir/$(basename -- "$0")"

# ---------------------------------------------------------------- defaults

worktree_arg=
session=
config_dir=
base_config=${ZELLIJ_CONFIG_DIR:-$HOME/.config/zellij}
layout_name=fujin
sidebar_width=32
resizable=0
theme=
minimal=0
do_build=1
do_grant=1
fresh=0
clean=0
nested=0
open_window=0
keep=0
terminal=auto
clean_all=0
dry_run=0

# extras/themes/*.kdl のファイル名（拡張子抜き）を --theme が受け付ける名前として列挙する
theme_names() {
  local f names=()
  for f in "$script_dir"/themes/*.kdl; do
    [ -f "$f" ] && names+=("$(basename "$f" .kdl)")
  done
  IFS=', '; printf '%s' "${names[*]}"
}

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
      --resizable     write the width as a percentage so zellij's resize works
      --theme NAME    append a theme from extras/themes/NAME.kdl and select it
                      (comments out any existing "theme" line first). Used to
                      check that fujin's colors follow zellij's theme instead
                      of hardcoding RGB (docs/issues/issue-agent-status-color-theme-variance.md).

      --no-build      skip `cargo build --release`
      --no-grant      do not pre-register the permissions (approve by hand instead)
      --fresh         kill an existing session of the same name first
      --open          open the session in a new terminal window and return
      --keep          with --open, do NOT clean up when the window's zellij exits
      --terminal CMD  terminal emulator for --open
                      (alacritty/wezterm/ghostty; default: alacritty)
      --nested        allow starting from inside a zellij session (nested)
      --clean         kill the session, delete the config dir, and exit
      --clean-all     do that for every fujin-try-* session, and exit

  -n, --dry-run       show what would happen, write nothing
  -h, --help          this message
EOS
  printf '                      available themes: %s\n' "$(theme_names)"
}

while [ $# -gt 0 ]; do
  case "$1" in
    -s|--session) session=$2; shift 2 ;;
    --config-dir) config_dir=$2; shift 2 ;;
    --base-config) base_config=$2; shift 2 ;;
    --minimal) minimal=1; shift ;;
    --width) sidebar_width=$2; shift 2 ;;
    --resizable) resizable=1; shift ;;
    --theme) theme=$2; shift 2 ;;
    --layout-name) layout_name=$2; shift 2 ;;
    --no-build) do_build=0; shift ;;
    --no-grant) do_grant=0; shift ;;
    --fresh) fresh=1; shift ;;
    --open) open_window=1; shift ;;
    --keep) keep=1; shift ;;
    --terminal) terminal=$2; open_window=1; shift 2 ;;
    --nested) nested=1; shift ;;
    --clean) clean=1; shift ;;
    --clean-all) clean_all=1; shift ;;
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

# zellij のサーバソケットは $TMPDIR/zellij-<uid>/contract_version_1/<セッション名>
# に作られ、Unix domain socket のパス上限（macOS で104バイト）に当たりうる
# （issue-try-open-picks-unlaunched-terminal.md「第二の問題」）。$TMPDIR はブート
# ごとに変わるので、ここで実測して budget を出す。
# **使えるのは103バイトまで**（104ではない）。zellij 側の判定は
# zellij-utils/src/cli.rs の validate_session で `パス長 >= ZELLIJ_SOCK_MAX_LENGTH`
# なので、104ちょうどは既に弾かれる。`contract_version_1` の数字は
# CLIENT_SERVER_CONTRACT_VERSION（0.44.3 で 1）
assert_session_name_fits() {
  # 上限値も socket dir の求め方も macOS 固有なので、他OSでは黙って見送る。
  # Linux は $XDG_RUNTIME_DIR 起点で桁数がまるで違い、誤検知するほうが害が大きい
  [ "$(uname -s)" = Darwin ] || return 0

  local sess=$1 sock_dir prefix budget
  if [ -n "${ZELLIJ_SOCKET_DIR:-}" ]; then
    # zellij はこの環境変数があれば $TMPDIR より優先する（envs::get_socket_dir）
    sock_dir=${ZELLIJ_SOCKET_DIR%/}
  else
    sock_dir=${TMPDIR:-/tmp}
    # Rust の PathBuf::push と同じく、末尾スラッシュは重ねない
    sock_dir="${sock_dir%/}/zellij-${UID:-$(id -u)}"
  fi
  prefix="$sock_dir/contract_version_1/"
  budget=$((103 - ${#prefix}))

  if [ "$budget" -le 0 ]; then
    die "no session name can fit: $prefix is already ${#prefix} bytes and a unix socket path caps at 104 bytes on macOS -- set a shorter TMPDIR (or ZELLIJ_SOCKET_DIR) for both this command and any later --clean"
  fi
  if [ "${#sess}" -gt "$budget" ]; then
    die "session name \"$sess\" is ${#sess} bytes but only $budget fit under $prefix (a unix socket path caps at 104 bytes on macOS) -- pass a shorter one with -s/--session, and the same -s to --clean afterwards"
  fi
}

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
theme_file=
if [ -n "$theme" ]; then
  theme_file="$script_dir/themes/$theme.kdl"
  [ -f "$theme_file" ] || die "no such theme: $theme (available: $(theme_names))"
fi

command -v zellij >/dev/null 2>&1 || die "zellij is not on PATH"

# ---------------------------------------------------------------- clean

# 一覧から1セッションぶんの行を取り出す。--no-formatting でも
# "<名前> [Created ...] (EXITED - attach to resurrect)" の形をしている
session_line() {
  zellij list-sessions --no-formatting 2>/dev/null | awk -v s="$1" '$1 == s'
}

# EXITED ではなく実際に動いているか。行が無ければ当然 false
session_is_running() {
  # 判定は zellij が出す実際のマーカーで行う（セッション名に EXITED を含む
  # ものを取り違えないため）
  session_line "$1" | grep -qv '(EXITED'
}

# **kill-session はサーバが畳み終わるのを待たない。** レイアウトのシリアライズ中に
# delete-session を撃つと、サーバ側が "Failed to dump layout" で panic する
# （zellij-server/src/lib.rs:2030。2026-08-18 の調査でログに20件）。
# 動いていない状態になるまで待ってから delete する
kill_session() {
  local target=${1:-$session} i
  zellij kill-session "$target" >/dev/null 2>&1 || true
  for i in 1 2 3 4 5 6 7 8 9 10; do
    session_is_running "$target" || break
    sleep 0.2
  done
  zellij delete-session "$target" >/dev/null 2>&1 || true
}

# 使い捨てセッションの EXITED は残しておく価値が無いので畳む。
# 対象は fujin-try-* だけに絞る（常駐セッションや手で作ったものには触らない）
purge_exited_try_sessions() {
  local name removed=0 exited
  exited=$(zellij list-sessions --no-formatting 2>/dev/null | grep '(EXITED' || true)
  [ -n "$exited" ] || return 0
  while read -r name _; do
    case "$name" in fujin-try-*) ;; *) continue ;; esac
    [ "$name" = "$session" ] && continue
    zellij delete-session "$name" >/dev/null 2>&1 || continue
    removed=$((removed + 1))
  done <<EOS
$exited
EOS
  if [ "$removed" -gt 0 ]; then
    info "deleted $removed exited fujin-try-* session(s)"
  fi
  return 0
}

# config dir を消してよいのは、このスクリプトが作ったものだけ。名前ではなく実体で照合する
assert_disposable_config_dir() {
  if [ "$1" = "$base_config" ] || [ "$1" = "$HOME/.config/zellij" ]; then
    die "refusing to remove $1 -- that is a real config dir"
  fi
}

if [ "$clean_all" -eq 1 ]; then
  step "Clean all  fujin-try-*"
  names=$(zellij list-sessions --no-formatting 2>/dev/null | awk '$1 ~ /^fujin-try-/ {print $1}')
  if [ "$dry_run" -eq 1 ]; then
    info "would kill and delete: ${names:-（該当なし）}"
    info "would remove $HOME/.config/zellij-fujin-*"
    exit 0
  fi
  for one in $names; do
    kill_session "$one"
    ok "killed and deleted $one"
  done
  [ -n "$names" ] || info "no fujin-try-* session was running"
  for one in "$HOME"/.config/zellij-fujin-*; do
    [ -d "$one" ] || continue
    assert_disposable_config_dir "$one"
    rm -rf "$one"
    ok "removed $one"
  done
  printf '\n'
  exit 0
fi

if [ "$clean" -eq 1 ]; then
  step "Clean  $session"
  if [ "$dry_run" -eq 1 ]; then
    info "would kill and delete the session $session"
    info "would remove $config_dir"
    exit 0
  fi
  kill_session
  ok "killed and deleted the session (if it existed)"
  assert_disposable_config_dir "$config_dir"
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

# **セッション名の検査は clean のあと。** 長すぎる名前で起動に失敗した人が最初に
# 叩くのは --clean なので、そこで die すると後始末の手段まで塞いでしまう。
# 名前を使うのはここから先（起動）だけなので、この位置で足りる
assert_session_name_fits "$session"

printf 'fujin try-worktree\n'
info "worktree    $worktree"
info "wasm        $wasm_path"
info "config dir  $config_dir"
info "session     $session"

# **生存確認は config dir を作り直す前に行う。** install_config は無条件に
# rm -rf $config_dir をするので、動作中のセッションがあるとその足元を消したうえ、
# 続く start_session が「セッションがある」と判断して attach してしまい、
# **同じセッションに2つ目の窓**ができる。どちらを閉じてもクライアントが道連れになる
session_running=0
if session_is_running "$session"; then
  session_running=1
fi

if [ "$session_running" -eq 1 ] && [ "$fresh" -eq 1 ]; then
  step "Fresh  $session"
  if [ "$dry_run" -eq 1 ]; then
    info "would kill the running session first (--fresh)"
  else
    kill_session
    ok "killed the previous session"
  fi
  session_running=0
fi

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

// 使い捨てなので復活させる必要が無い。切っておくと delete-session との競合
// （サーバ側の "Failed to dump layout" panic）も EXITED セッションの溜まりも起きない
session_serialization false
EOS
}

install_config() {
  step "Config dir  $config_dir"

  if [ "$session_running" -eq 1 ]; then
    warn "$session is still running -- leaving $config_dir untouched"
    info "the running server reads this dir, and recreating it would also make"
    info "the next step attach a *second* window to the same session."
    info "run --clean (or --fresh) first if you want a rebuilt config dir"
    return 0
  fi

  if [ "$dry_run" -eq 1 ]; then
    info "would recreate $config_dir"
    if [ "$minimal" -eq 0 ] && [ -f "$base_config/config.kdl" ]; then
      info "would copy $base_config (minus plugins/) and point the \"$layout_name\" alias at the worktree build"
    else
      info "would write a minimal config.kdl"
    fi
    if [ -n "$theme" ]; then
      info "would comment out any existing \"theme\" line and append theme \"$theme\" from $theme_file"
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

    # 使い捨ての config dir では resurrection を切る。ベース側で有効になっていても
    # ここで後から宣言したものが効く（重複を残さないよう既存の有効行は潰す）
    if grep -qE '^[[:space:]]*session_serialization[[:space:]]' "$config_file"; then
      local tmp_ss="$config_file.ss.$$"
      sed -E 's/^([[:space:]]*)(session_serialization[[:space:]])/\1\/\/ \2/' \
        "$config_file" >"$tmp_ss"
      mv "$tmp_ss" "$config_file"
    fi
    printf '\n// try-worktree.sh: 使い捨てセッションなので復活させない\nsession_serialization false\n' \
      >>"$config_file"

    # エイリアスの location だけを worktree のビルドへ向け直す。
    # 1行の置換で済むのは、config.kdl 側が決定202608022309でエイリアスに寄せてあるため
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
  local -a layout_args
  layout_args=(--layout-only --force
    --config-dir "$config_dir"
    --plugin-dir "$(dirname "$wasm_path")"
    --layout-name "$layout_name"
    --width "$sidebar_width")
  [ "$resizable" -eq 1 ] && layout_args+=(--resizable)
  "$script_dir/setup.sh" "${layout_args[@]}" >/dev/null
  ok "wrote $layout_file"

  # **`&&` の右辺で終える形にしない。** これが関数の最後の文だと、`$theme` が
  # 空のときの終了ステータス（左辺の失敗）がそのまま install_config の戻り値になり、
  # 呼び出し側が if/while 等で囲んでいない bare 呼び出しのため set -e が発火して
  # 後続の grant_permissions 以降が無言でスキップされる（2026-08-13 実測）
  if [ -n "$theme" ]; then
    apply_theme
  fi
}

# --theme が指定されたときだけ呼ばれる。config.kdl 末尾に extras/themes/<name>.kdl の
# themes ブロックと theme 宣言を追記し、既存の有効な theme 行はコメントアウトする
# （2つ以上残すとどちらが効くか zellij のパース順まかせになる）
apply_theme() {
  if grep -qE '^[[:space:]]*theme[[:space:]]+"' "$config_file"; then
    local tmp="$config_file.theme.$$"
    sed -E 's/^([[:space:]]*)(theme[[:space:]]+")/\1\/\/ \2/' "$config_file" >"$tmp"
    mv "$tmp" "$config_file"
  fi
  {
    printf '\n// --theme %s (appended by extras/try-worktree.sh from %s)\n' "$theme" "$theme_file"
    printf 'theme "%s"\n\n' "$theme"
    cat "$theme_file"
  } >>"$config_file"
  ok "appended theme \"$theme\" from $theme_file"
}

# ---------------------------------------------------------------- permissions

# 承認は「展開後の wasm 絶対パス」単位で cache dir の permissions.kdl に記録される。
# config dir を分けても cache dir は共通なので、ここへ先に書いておけば
# 初回の承認プロンプト（7種を手で承認する手間）を踏まずに済む。
# zellij は起動時にこのファイルを読み、実際に要求された権限だけを書き戻すので、
# 余分に書いても消えるだけで害はない（2026-08-09 実測）。
# この worktree のビルドに対して既に承認されている権限を1行1つで返す。
# エントリの書式は `"<path>" {` ... `}`（zellij が書き戻す形）
entry_permissions() {
  [ -f "$1" ] || return 0
  awk -v head="\"$wasm_path\" {" '
    index($0, head) == 1 { inside = 1; next }
    inside && /^}/ { inside = 0; next }
    inside { gsub(/[[:space:]]/, ""); if ($0 != "") print }
  ' "$1" | sort -u
}

# 同じ worktree のエントリだけを取り除いたファイル内容を返す
strip_entry() {
  awk -v head="\"$wasm_path\" {" '
    index($0, head) == 1 { skip = 1; next }
    skip && /^}/ { skip = 0; next }
    !skip
  ' "$1"
}

grant_permissions() {
  step "Permissions"

  local cache_dir perms
  cache_dir=$(zellij setup --check 2>/dev/null | sed -n 's/^\[CACHE DIR\]: *//p' | tr -d '"')
  [ -n "$cache_dir" ] || { warn "could not determine the cache dir; skipping"; return 0; }
  perms="$cache_dir/permissions.kdl"

  # 要求する権限は wasm 側の正本（src/main.rs）から拾う。手で並べると
  # 権限が増えたとき（決定202608082045 の ReadPaneContents など）に取り残される
  local types
  types=$(grep -oE 'PermissionType::[A-Za-z]+' "$worktree/src/main.rs" 2>/dev/null \
    | sed 's/PermissionType:://' | sort -u)
  [ -n "$types" ] || { warn "could not read the permission list from src/main.rs; skipping"; return 0; }

  # 判定はパスの有無ではなく**中身**で行う。エントリだけ先にあって権限が
  # 足りない状態（前回の実行のあと fujin が新しい権限を要求するようになった場合）
  # を見落とすと、未承認が1つ残るだけでサイドバーは承認プロンプトすら出さずに
  # 空白のまま描画される（docs/issues/issue-sidebar-blank-in-fresh-session.md）
  local granted missing
  granted=$(entry_permissions "$perms")
  missing=$(comm -23 <(printf '%s\n' "$types") <(printf '%s\n' "$granted"))

  if [ -z "$missing" ]; then
    ok "already granted for this worktree's build"
    return 0
  fi

  if [ "$dry_run" -eq 1 ]; then
    [ -n "$granted" ] && info "the entry exists but is missing: $(printf '%s ' $missing)"
    info "would write to $perms:"
    printf '    | "%s" {\n' "$wasm_path"
    printf '    |     %s\n' $types
    printf '    | }\n'
    return 0
  fi

  mkdir -p "$cache_dir"
  # 権限が増えていた場合は追記でなく置き換える。同じパスのエントリを
  # 2つ持たせると、どちらが効くかが zellij のパース順まかせになる
  if [ -n "$granted" ]; then
    cp "$perms" "$perms.bak"
    strip_entry "$perms" >"$perms.new" && mv "$perms.new" "$perms"
    info "replaced the existing entry (backup: $perms.bak)"
  fi
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
  for t in ${FUJIN_TERMINAL:-} ${TERMINAL:-} alacritty wezterm ghostty; do
    if command -v "$t" >/dev/null 2>&1; then printf '%s' "$t"; return 0; fi
  done
  return 1
}

start_session() {
  step "Session  $session"

  if [ "$dry_run" -eq 0 ]; then
    purge_exited_try_sessions
  fi

  local exists=0
  zellij list-sessions --no-formatting 2>/dev/null | awk '{print $1}' | grep -qx "$session" && exists=1

  # 動作中のセッションは既に上（session_running の判定）で畳んである。
  # ここに残るのは EXITED のまま一覧に居座っているものだけ
  if [ "$exists" -eq 1 ] && [ "$fresh" -eq 1 ]; then
    if [ "$dry_run" -eq 1 ]; then
      info "would delete the leftover session first (--fresh)"
    else
      kill_session
      ok "deleted the leftover session (--fresh)"
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

    # **--clean は既定名を組み立て直す。** -s / --config-dir を明示されていた場合、
    # 素の `--clean <worktree>` を案内すると *別の*（既定名の）セッションと config dir
    # を畳みに行く。セッション名が長すぎるときの回避策が -s なので、案内する側が
    # それを落とさないようにする
    local clean_cmd="$self_sh --clean ${worktree_arg:-$name}"
    [ "$session" = "fujin-try-$name" ] || clean_cmd="$clean_cmd -s $session"
    [ "$config_dir" = "$HOME/.config/zellij-fujin-$name" ] ||
      clean_cmd="$clean_cmd --config-dir $config_dir"

    # 「このコマンドを実行しろ」の渡し方はターミナルごとに違う。
    # wezterm だけサブコマンド形式で、残りは -e で揃う
    local -a exec_args
    case "$(basename "$term")" in
      wezterm) exec_args=(start --) ;;
      *) exec_args=(-e) ;;
    esac

    # **窓の中身を zellij 単体にしない。** それだと窓の中に「終わらせ方」が無く、
    # ×ボタンで閉じるしか道が残らない。zellij 0.44.3 はその閉じ方で
    # クライアントが panic し、配下のシェルが孤児化して CPU を食う
    # （docs/issues/issue-window-close-panics-orphan-shell.md）。
    # シェルで包んでおけば、正常終了・detach・窓を閉じたときの SIGHUP の
    # どれでも後始末（セッションと config dir の破棄）を通せる
    local -a launch
    if [ "$keep" -eq 1 ]; then
      launch=("${env_prefix[@]}" "${cmd[@]}")
    else
      launch=("${env_prefix[@]}"
        FUJIN_TRY_SH="$self_sh" FUJIN_TRY_WT="${worktree_arg:-$name}"
        FUJIN_TRY_SESSION="$session" FUJIN_TRY_CFG="$config_dir"
        sh -c 'cleanup() {
  # trap 経由と最後の1行と、両方から呼ばれるので冪等にしておく
  if [ -z "${FUJIN_TRY_CLEANED-}" ]; then
    FUJIN_TRY_CLEANED=1
    # -s / --config-dir は必ず渡す。省くと --clean 側が既定名を組み立て直すので、
    # 明示指定で起動していた場合に別のセッション・config dir を消してしまう
    "$FUJIN_TRY_SH" --clean "$FUJIN_TRY_WT" \
      -s "$FUJIN_TRY_SESSION" --config-dir "$FUJIN_TRY_CFG" >/dev/null 2>&1 || true
  fi
}
trap cleanup HUP TERM INT
"$@"
cleanup' sh "${cmd[@]}")
    fi

    # **成否は握り潰さない。** 以前は出力を /dev/null に捨て、起動の成否を見ずに
    # ok を出していたため、窓が開かなくても「開いた」と報告していた
    # （issue-try-open-picks-unlaunched-terminal.md）。$term の出力は残し、
    # セッションが実際に現れるまで少し待ってから成否を判定する。
    # **ログの置き場所は config dir の外。** 窓の中の zellij が即死する類の失敗だと、
    # 包んだシェルの cleanup が --clean を呼んで config dir ごと消してしまい、
    # 下でログを読むころには残っていない
    local log_dir=${TMPDIR:-/tmp}
    local log_file="${log_dir%/}/fujin-try-$name-open.log"
    : >"$log_file"
    nohup "$term" "${exec_args[@]}" "${launch[@]}" >"$log_file" 2>&1 &
    disown

    # $term の pid で早期打ち切りはしない: GUIアプリはlaunchd経由で再親化され、
    # 元プロセスが即終了しても窓は後から開くことがあるため「死んだ=失敗」と
    # 判定できない。素直にタイムアウトまでセッションの出現だけを見る。
    # ターミナルのコールドスタート込みで実測1〜2秒なので、10秒あれば足りる
    local timeout_s=10 tick=0 ticks launched=0
    ticks=$((timeout_s * 5))
    while [ "$tick" -lt "$ticks" ]; do
      if session_is_running "$session"; then
        launched=1
        break
      fi
      sleep 0.2
      tick=$((tick + 1))
    done

    if [ "$launched" -eq 1 ]; then
      ok "opened a new $term window running \"$session\""
      if [ "$keep" -eq 1 ]; then
        info "--keep: nothing is cleaned up automatically"
        info "close it with: $clean_cmd"
      else
        info "the session and $config_dir are dropped when that window's zellij exits"
        info "to tear it down from here: $clean_cmd"
      fi
      return 0
    fi

    # **ここで 0 を返さない。** 窓が開いていない以上これは失敗で、`make try` を
    # 緑にしてしまうと「報告だけ成功していて実体が無い」元の症状に逆戻りする
    warn "\"$session\" did not come up within ${timeout_s}s -- the $term window probably never opened"
    if [ -s "$log_file" ]; then
      info "$term wrote:"
      sed 's/^/        /' "$log_file"
    else
      info "$term wrote nothing (log: $log_file)"
    fi
    info "try another terminal with --terminal CMD, or open one yourself and run:"
    printf '\n        ZELLIJ_CONFIG_DIR=%s %s\n\n' "$config_dir" "${cmd[*]}"
    info "clean up whatever is left with: $clean_cmd"
    exit 1
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
