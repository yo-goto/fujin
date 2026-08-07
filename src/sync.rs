// インスタンス間の状態同期（決定13）。
//
// pipe は起動中の全インスタンスに届くが、**後から起動したインスタンスは
// それ以前のイベントを見ていない**。タブを後から作ると、そのサイドバーだけ
// アイコンが出ない/古いという食い違いになる。そこで既存インスタンスが
// 新入りを見つけて状態を押し付ける。
//
// 逆（新入りが要求を投げる）にしてはいけない。宛先をURLで指定する
// `MessageToPlugin::with_plugin_url` は、起動中のインスタンスに配送されず
// **新しいプラグインを起動しようとする**（cwd/config まで一致を要求するため、
// 常駐サイドバーにマッチしない）。実測でも
// `wasm_bridge.rs:1897 Failed to load plugin` が出て、実セッションなら
// タブを作るたびに迷子のサイドバーペインが増えるところだった。
// 宛先をプラグインIDで直接指定すれば起動は起こらない。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::agent::{AgentInfo, AgentState};
use crate::{State, COMMAND_STATE_PIPE, READ_CLEAR_PIPE, SELECTION_PIPE, SYNC_STATE_PIPE};

impl State {
    // 自分のwasm URLを知る（get_plugin_ids() には入っていない）
    pub(crate) fn learn_own_plugin_url(&mut self) {
        if self.own_plugin_url.is_some() {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        // 一覧から引ければそれでよいが、非可視インスタンスには PaneUpdate が
        // 届かないので、それだけでは永久に埋まらない。埋まらないまま入場pipeを
        // 受けると召喚（決定16）が「own id/url unknown」で不発になる（実測）。
        // `get_pane_info()` はサーバへの問い合わせなので可視性に依らない
        self.own_plugin_url = self
            .panes
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .panes
                    .values()
                    .flatten()
                    .find(|p| p.is_plugin && p.id == own_id)
            })
            .and_then(|p| p.plugin_url.clone())
            .or_else(|| get_pane_info(PaneId::Plugin(own_id)).and_then(|p| p.plugin_url));
    }

    // 新しく現れた兄弟インスタンス（同じURLのプラグインペイン）に状態を配る。
    // 状態を持っているインスタンスは全員が送るが、受け手は空のときしか
    // 取り込まないので重複しても害はない。リーダー選出は不要
    pub(crate) fn push_state_to_new_siblings(&mut self) {
        let (Some(own_id), Some(own_url), Some(manifest)) = (
            self.own_plugin_id,
            self.own_plugin_url.as_deref(),
            self.panes.as_ref(),
        ) else {
            return;
        };
        let siblings: BTreeSet<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| p.is_plugin && p.id != own_id && p.plugin_url.as_deref() == Some(own_url))
            .map(|p| p.id)
            .collect();
        let newcomers: Vec<u32> = siblings.difference(&self.known_siblings).copied().collect();
        self.known_siblings = siblings;
        if newcomers.is_empty() {
            return;
        }
        let dump = (!self.agents.is_empty()).then(|| self.state_dump());
        // コマンド状態も一緒に配る（決定32）。導出できるのは PaneUpdate が届く
        // このインスタンスだけなので、新入りは押し付けられない限り一生知らない
        let commands = (!self.commands.is_empty()).then(|| self.command_dump());
        for id in newcomers {
            if let Some(dump) = &dump {
                pipe_message_to_plugin(
                    MessageToPlugin::new(SYNC_STATE_PIPE)
                        .with_destination_plugin_id(id)
                        .with_payload(dump.clone()),
                );
            }
            if let Some(commands) = &commands {
                pipe_message_to_plugin(
                    MessageToPlugin::new(COMMAND_STATE_PIPE)
                        .with_destination_plugin_id(id)
                        .with_payload(commands.clone()),
                );
            }
        }
    }

    // 1ペイン1行のTSV。区切りにタブと改行を使うのは、パス（cwd）にも
    // エージェント名にも現れないため
    pub(crate) fn state_dump(&self) -> String {
        let mut out = String::new();
        for (pane_id, info) in &self.agents {
            let cwd = self
                .pane_cwds
                .get(pane_id)
                .map(|s| s.as_str())
                .unwrap_or("");
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                pane_id,
                info.state.as_str(),
                info.subagents,
                info.open_tasks,
                info.agent,
                cwd,
                info.turn_ended as u8,
                // シーケンス番号も運ぶ。無いと、後から起動したインスタンスの
                // トリアージ一覧で同一階層内の並びが総崩れになる（全員0で
                // ツリー順に潰れる）
                info.state_change_seq
            ));
        }
        out
    }

    pub(crate) fn apply_state_dump(&mut self, raw: &str) {
        for line in raw.lines() {
            let mut fields = line.split('\t');
            let (Some(pane_id), Some(state), Some(subagents), Some(open_tasks), Some(agent)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let Ok(pane_id) = pane_id.parse::<u32>() else {
                continue;
            };
            let cwd = fields.next();
            let turn_ended = fields.next() == Some("1");
            let state_change_seq = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            // 自分のカウンタを配られた最大値まで進めておく。以降に自分が振る
            // 番号が取り込んだものより古くなると、順序が逆転する
            self.state_seq = self.state_seq.max(state_change_seq);
            self.agents.insert(
                pane_id,
                AgentInfo {
                    state: AgentState::from_str(state),
                    agent: agent.to_string(),
                    subagents: subagents.parse().unwrap_or(0),
                    open_tasks: open_tasks.parse().unwrap_or(0),
                    turn_ended,
                    detail: None,
                    state_change_seq,
                },
            );
            if let Some(cwd) = cwd.filter(|c| !c.is_empty()) {
                self.pane_cwds.insert(pane_id, cwd.to_string());
            }
        }
        // 既に閉じたペインの状態が混ざらないようにする
        self.prune_stale_agents();
        // 起動ごとに高々1回。食い違いを追うときの手がかりになるので残す
        eprintln!("fujin: synced {} agents from peer", self.agents.len());
    }

    // 選択を兄弟インスタンスへ配る（運搬形式は選択ペインID）。navモード中の
    // 移動は横取り中の1インスタンスにしか起きないため、これがないと
    // タブごとに違う行が光る
    pub(crate) fn broadcast_selection(&self) {
        let Some(entry) = self.selectable.get(self.selected) else {
            return;
        };
        let pane_id = entry.pane_id;
        for sibling in &self.known_siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new(SELECTION_PIPE)
                    .with_destination_plugin_id(*sibling)
                    .with_payload(pane_id.to_string()),
            );
        }
    }

    pub(crate) fn broadcast_read_clears(&self, pane_ids: &[u32]) {
        if pane_ids.is_empty() {
            return;
        }
        let payload = pane_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        for sibling in &self.known_siblings {
            pipe_message_to_plugin(
                MessageToPlugin::new(READ_CLEAR_PIPE)
                    .with_destination_plugin_id(*sibling)
                    .with_payload(payload.clone()),
            );
        }
    }
}
