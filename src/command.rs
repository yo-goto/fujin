// コマンド状態の管理（決定202608072218。要件: .docs/requirements/req-command-status.md）。
//
// コマンドペインの走行・終了を、エージェント状態と同じ記号でサイドバーに出す。
// 別概念で、持つ値は `working` / `done` / `error` の3値だけ。
//
// **検知は `PaneManifest` からの導出で行う。** `CommandPane*` イベントは
// そのペインを自分で開いたプラグインにしか配送されない（zellij 0.44.3 の
// `zellij-server/src/pty.rs`）ため、利用者が開いたペインには使えない。
// `PaneUpdate` は可視インスタンスにしか届かないので導出できるのは権威
// インスタンス1つだけで、観測した変化は兄弟インスタンスへ配る。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::agent::AgentState;
use crate::{State, COMMAND_STATE_PIPE};

// コマンド状態の3値。`idle` / `blocked` は持たない — コマンドペインに
// 「許可待ち」の概念は無く、既読は状態を持たない側へ戻すことで表す（決定202608072218）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandState {
    Working,
    Done,
    Error,
}

impl CommandState {
    // 記号・色・優先度階層はエージェント状態と共用する（決定202608072218）。概念は別だが、
    // 待ち件数・トリアージ一覧へ統合する以上、記号を分けると凡例が倍増する。
    // 対応表を二重に持たないよう、同じ意味のエージェント状態へ委譲する
    fn as_agent_state(self) -> AgentState {
        match self {
            CommandState::Working => AgentState::Working,
            CommandState::Done => AgentState::Done,
            CommandState::Error => AgentState::Error,
        }
    }

    // インスタンス間同期のワイヤ表現（決定202608012141）
    pub(crate) fn as_str(self) -> &'static str {
        self.as_agent_state().as_str()
    }

    // 解釈できない値は `working` に倒す。走っていないものを走っていると
    // 見せるほうが、終わったものを見落とすより実害が小さい
    pub(crate) fn from_str(s: &str) -> Self {
        match s {
            "done" => CommandState::Done,
            "error" => CommandState::Error,
            _ => CommandState::Working,
        }
    }

    // 終了コードの解釈（決定202608072218）。`Some(0)` だけが `done` で、非0・シグナル
    // 終了・ユーザーによる中断は区別せず一律 `error`。中断で `error` が出ても
    // 既読モデルでフォーカスすれば消えるだけなので、賢い自動判定はしない
    fn from_exit(exited: bool, exit_status: Option<i32>) -> Self {
        if !exited {
            return CommandState::Working;
        }
        if exit_status == Some(0) {
            CommandState::Done
        } else {
            CommandState::Error
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CommandInfo {
    pub(crate) state: CommandState,
    // 直近に状態が変わったときのシーケンス番号（エージェント状態と同じ用途。
    // トリアージ一覧の同一階層内の tie-break に使う）
    pub(crate) state_change_seq: u64,
    // 既読か（決定202608072218）。コマンド状態には戻り先の `idle` が無いので、既読は
    // 「状態を持たない」＝アイコンを出さないことで表す。
    //
    // **旗で持つ必要がある。** 状態そのものを捨てると、終了したコマンドペインは
    // 次の PaneUpdate で `exited` から同じ状態が再導出されて復活してしまう
    pub(crate) read: bool,
    // 既読の猶予（.docs/issues/issue-command-status-error-icon-swallowed.md）。
    //
    // **状態が付いた瞬間にそのペインがフォーカスされていたら、その状態は
    // フォーカスが一度離れて戻ってくるまで既読にしない。** `zellij run` は
    // 新しいペインへフォーカスを移すので、一瞬で終わるコマンドは必ず
    // 「フォーカス中に終了」する。素直に既読モデルを当てると、状態が付いた
    // 同じ PaneUpdate の中で既読になり、アイコンが一度も描かれないまま消える
    //（実測: `sh -c 'exit 1'` で 0ms）。
    //
    // 兄弟インスタンスへは配らない。既読モデルを回すのは PaneUpdate が届く
    // 可視インスタンスだけで、この旗はそこでの観測から毎回引き直せる
    pub(crate) awaiting_refocus: bool,
}

// エージェント状態とコマンド状態を、描画・集計の手前で1つに畳んだ見え方。
//
// 状態を出す側（ペイン行・トリアージ行・待ち件数）にどちらのソースかを
// 意識させないための型。ソースの優先順位は `State::pane_status` が畳む
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneStatus {
    Agent(AgentState),
    Command(CommandState),
}

impl PaneStatus {
    fn as_agent_state(self) -> AgentState {
        match self {
            PaneStatus::Agent(state) => state,
            PaneStatus::Command(state) => state.as_agent_state(),
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        self.as_agent_state().icon()
    }

    pub(crate) fn color(self) -> usize {
        self.as_agent_state().color()
    }

    pub(crate) fn triage_rank(self) -> Option<u8> {
        self.as_agent_state().triage_rank()
    }

    pub(crate) fn is_waiting(self) -> bool {
        self.as_agent_state().is_waiting()
    }
}

impl State {
    // ペインに出す状態（決定202608072218）。
    //
    // **エージェント登録があるペインは常にそちらが優先**で、コマンド状態は
    // 無視する（`zellij run -- claude` のようにコマンドペイン経由でエージェントを
    // 起動したケース）。フックはターン単位の意味のある遷移を伝えるのに対し、
    // コマンドペイン側の終了検知は解像度が粗く、両方が同じペインに出ると競合する
    pub(crate) fn pane_status(&self, pane_id: u32) -> Option<PaneStatus> {
        if let Some(agent) = self.agents.get(&pane_id) {
            return Some(PaneStatus::Agent(agent.state));
        }
        let info = self.commands.get(&pane_id)?;
        if info.read {
            return None;
        }
        Some(PaneStatus::Command(info.state))
    }

    // 直近に状態が変わったシーケンス番号（トリアージ一覧の tie-break 用）。
    // 優先順位は `pane_status` と揃える
    pub(crate) fn status_seq(&self, pane_id: u32) -> u64 {
        if let Some(agent) = self.agents.get(&pane_id) {
            return agent.state_change_seq;
        }
        self.commands
            .get(&pane_id)
            .map(|info| info.state_change_seq)
            .unwrap_or(0)
    }

    // コマンドペインの状態を PaneManifest から導出する。
    //
    // 絞り込みは行わない（決定202608072218）。`ls` のような些末なコマンドでも、
    // コマンドペインとして開かれた以上は無条件で追跡する — 実行時間の閾値等の
    // ヒューリスティックは決定202607310310で放棄した「賢い自動判定」の二の舞になりやすく、
    // 既読モデルがある以上、実害は一瞬アイコンが増える程度に留まる
    pub(crate) fn apply_command_states(&mut self, manifest: &PaneManifest) {
        let mut changed = Vec::new();
        for pane in manifest.panes.values().flatten() {
            if pane.is_plugin || pane.is_suppressed {
                continue;
            }
            // コマンドペインでなければ状態を持たない（フックが無ければ状態を
            // 持たないエージェント状態と対称の設計原理）
            if pane.terminal_command.is_none() {
                continue;
            }
            let next = CommandState::from_exit(pane.exited, pane.exit_status);
            if self.commands.get(&pane.id).map(|info| info.state) == Some(next) {
                // 同じ状態のままなら既読も据え置く。ここで作り直すと、既読に
                // したはずの `done` が PaneUpdate のたびに復活する
                continue;
            }
            // 再実行（`ReRun`）はここを通って `working` に戻る。既読も落として、
            // 次の終了をまた拾えるようにする
            self.state_seq += 1;
            self.commands.insert(
                pane.id,
                CommandInfo {
                    state: next,
                    state_change_seq: self.state_seq,
                    read: false,
                    // 状態が付いた瞬間にフォーカスしていたなら、既読はフォーカスが
                    // 一度離れて戻るまで待つ
                    //（.docs/issues/issue-command-status-error-icon-swallowed.md）
                    awaiting_refocus: pane.is_focused,
                },
            );
            changed.push(pane.id);
        }
        self.broadcast_command_states(&changed);
    }

    // 閉じられた／コマンドペインでなくなったペインの状態を破棄
    pub(crate) fn prune_stale_commands(&mut self, live_commands: &BTreeSet<u32>) {
        self.commands.retain(|id, _| live_commands.contains(id));
    }

    // 観測した変化を兄弟インスタンスへ配る（決定202608012141）。
    //
    // 導出できるのは PaneUpdate が届く可視インスタンスだけなので、配らないと
    // 他のタブのサイドバーは何も知らないままになる。**既読も配る必要がある** —
    // 終了したコマンドペインは `exited` が立ちっぱなしなので、既読を知らない
    // インスタンスが可視になった瞬間に同じ `done` を導出し直してしまう
    fn broadcast_command_states(&self, pane_ids: &[u32]) {
        if pane_ids.is_empty() || self.known_siblings.is_empty() {
            return;
        }
        let mut payload = String::new();
        for pane_id in pane_ids {
            let Some(info) = self.commands.get(pane_id) else {
                continue;
            };
            payload.push_str(&command_line(*pane_id, info));
        }
        self.broadcast_to_siblings(COMMAND_STATE_PIPE, &payload);
    }

    // 新入りインスタンスへ押し付けるコマンド状態のダンプ（決定202608012141）。
    // エージェント状態のダンプ（`state_dump`）と同じく1ペイン1行のTSV
    pub(crate) fn command_dump(&self) -> String {
        let mut out = String::new();
        for (pane_id, info) in &self.commands {
            out.push_str(&command_line(*pane_id, info));
        }
        out
    }

    // fujin_command の受け口
    pub(crate) fn handle_command_state_pipe(&mut self, payload: Option<&str>) -> bool {
        payload
            .map(|raw| self.apply_command_dump(raw))
            .unwrap_or(false)
    }

    // 配られたコマンド状態を取り込む。戻り値は再描画するか。
    //
    // 送り手は PaneUpdate を受け取れる可視インスタンス＝導出の権威なので、
    // 既に持っている行も上書きしてよい
    pub(crate) fn apply_command_dump(&mut self, raw: &str) -> bool {
        let mut changed = false;
        for line in raw.lines() {
            let mut fields = line.split('\t');
            let (Some(pane_id), Some(state), Some(read), Some(seq)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Ok(pane_id) = pane_id.parse::<u32>() else {
                continue;
            };
            let state_change_seq = seq.parse().unwrap_or(0);
            // 自分のカウンタも配られた最大値まで進める（トリアージ一覧の
            // 同一階層内の並びが逆転しないように）
            self.state_seq = self.state_seq.max(state_change_seq);
            self.commands.insert(
                pane_id,
                CommandInfo {
                    state: CommandState::from_str(state),
                    state_change_seq,
                    read: read == "1",
                    // 猶予は配らない（受け手は既読モデルを回さない）。可視に
                    // なった時点で自前の観測から引き直される
                    awaiting_refocus: false,
                },
            );
            changed = true;
        }
        changed
    }
}

// 同期のワイヤ表現1行ぶん。区切りはエージェント状態のダンプと揃えてタブ
fn command_line(pane_id: u32, info: &CommandInfo) -> String {
    format!(
        "{}\t{}\t{}\t{}\n",
        pane_id,
        info.state.as_str(),
        info.read as u8,
        info.state_change_seq
    )
}

impl CommandInfo {
    // 既読化（決定202608072218）。`working` は既読にならない — 走っている最中の
    // コマンドは人の対応を待っていない。
    //
    // 猶予（`awaiting_refocus`）が立っている間も既読にしない。解くのは
    // `State::release_read_grace`（フォーカスが離れたことの観測）だけで、
    // ここでは判定に使うだけに留める — 兄弟インスタンスから配られた既読
    // クリアはこの経路を通るので、猶予で握り潰してはいけない…という誤りを
    // 避けるため、配布の受け口は `force_read` を使う
    pub(crate) fn mark_read(&mut self) -> bool {
        if self.awaiting_refocus {
            return false;
        }
        self.force_read()
    }

    // 猶予を無視して既読にする。兄弟インスタンスから配られた既読クリア用
    //（送り手の可視インスタンスが猶予込みで判断済みなので、受け手は従う）
    pub(crate) fn force_read(&mut self) -> bool {
        if !self.is_unread() {
            return false;
        }
        self.read = true;
        true
    }

    // まだ既読にしていない注意を引く状態を持っているか（猶予は見ない）
    pub(crate) fn is_unread(&self) -> bool {
        !self.read && matches!(self.state, CommandState::Done | CommandState::Error)
    }
}

impl State {
    // 既読の猶予を解く（.docs/issues/issue-command-status-error-icon-swallowed.md）。
    //
    // アクティブタブでフォーカスされていないコマンドペインは「ユーザーが離れた」
    // とみなす。別タブのコマンドペインもここに含まれる（フォーカスはアクティブ
    // タブにしか無い）ので、タブを切り替えて戻ってきた場合も猶予は解けている。
    //
    // フォーカスの有無だけで判定できるのは、猶予が「まだ離れていない」ことしか
    // 意味しないため。離れた瞬間を捕まえる必要はない
    pub(crate) fn release_read_grace(&mut self, focused: &BTreeSet<u32>) {
        for (pane_id, info) in self.commands.iter_mut() {
            if !focused.contains(pane_id) {
                info.awaiting_refocus = false;
            }
        }
    }
}

// コマンドペインの表示名（決定202608072218）。ペイン名が空のときだけコマンド文字列を
// 代わりに出す（ペイン名フォールバック）。`zellij run --name` やリネームで
// 名前が付いているペインはそちらを優先する
pub(crate) fn fallback_title(pane: &PaneInfo) -> String {
    if !pane.title.trim().is_empty() {
        return pane.title.clone();
    }
    pane.terminal_command.clone().unwrap_or_default()
}

// PaneManifest に居るコマンドペインのID（prune 用）
pub(crate) fn live_command_ids(manifest: &PaneManifest) -> BTreeSet<u32> {
    manifest
        .panes
        .values()
        .flatten()
        .filter(|p| !p.is_plugin && p.terminal_command.is_some())
        .map(|p| p.id)
        .collect()
}
