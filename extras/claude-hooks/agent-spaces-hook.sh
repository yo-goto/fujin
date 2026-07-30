#!/usr/bin/env bash
# agent-spaces: Claude Code フック → zellij pipe ブリッジ
#
# stdin で受けたフックJSONを最小のペイロードに変換し、稼働中の
# agent-spaces プラグインへ届ける。
#
# 重要（docs/04-design-decisions.md リスク1）:
#   `--plugin` は絶対に付けないこと。付けると未起動のプラグインを
#   勝手に起動してしまう。--plugin なしなら起動中のプラグインにのみ
#   配送され、未起動時は完全な no-op になる。

# zellij ペイン外（例: 素のターミナル）では何もしない
[ -n "$ZELLIJ_PANE_ID" ] || exit 0

payload=$(jq -c '{
  pane_id: (env.ZELLIJ_PANE_ID | tonumber),
  agent: "claude",
  event: .hook_event_name,
  cwd: (.cwd // empty),
  detail: (.message // empty)
}' 2>/dev/null) || exit 0

[ -n "$payload" ] || exit 0

exec zellij pipe --name agent_spaces_status -- "$payload"
