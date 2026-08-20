#!/usr/bin/env bash
# fujin: Claude Code フック → zellij pipe ブリッジ
#
# stdin で受けたフックJSONを最小のペイロードに変換し、稼働中の
# fujin プラグインへ届ける。
#
# 重要（docs/concept/design-decisions.md リスク1）:
#   `--plugin` は絶対に付けないこと。付けると未起動のプラグインを
#   勝手に起動してしまう。--plugin なしなら起動中のプラグインにのみ
#   配送され、未起動時は完全な no-op になる。

# zellij ペイン外（例: 素のターミナル）では何もしない
[ -n "$ZELLIJ_PANE_ID" ] || exit 0

# 注意: `key: (.foo // empty)` と書いてはいけない。jq のオブジェクト構築で
# 値が empty になると、そのキーどころか**オブジェクト全体が消える**。
# `.message` を持たないイベント（Notification 以外すべて）で payload が空になり、
# 状態がまったく届かなくなる。null を入れておいて後段で落とす。
# `source` は SessionStart にだけ付く（startup / resume / clear / compact / fork）。
# fujin は配置演出のトリガー判定に使う — clear・compact は稼働中のエージェントの
# 仕切り直しなので、新規エージェント検出から除外する必要がある
payload=$(jq -c '{
  pane_id: (env.ZELLIJ_PANE_ID | tonumber),
  agent: "claude",
  event: .hook_event_name,
  source: .source,
  cwd: .cwd,
  detail: .message
} | with_entries(select(.value != null))' 2>/dev/null) || exit 0

[ -n "$payload" ] || exit 0

exec zellij pipe --name fujin_status -- "$payload"
