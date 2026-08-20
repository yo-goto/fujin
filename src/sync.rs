// インスタンス間の状態同期（決定202608012141）。
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
//
// ただし押し付けだけでは届かない（docs/issues/issue-tab-switch-agent-status-desync.md）。
// 新しいタブができた瞬間は**新入りが可視・既存が全員非可視**なので、押し付けの
// 起点である `PaneUpdate` が既存側で発火しない。そこで新入りは自分が兄弟を
// 見つけた時点で**状態ダンプを要求する**（宛先はプラグインID指定なので上記の
// 起動事故は起きない。幅で先に同じ穴を埋めた `WIDTH_REQUEST` と同じ形）。

use std::collections::BTreeSet;

use zellij_tile::prelude::*;

use crate::agent::{AgentInfo, AgentState};
use crate::host;
use crate::{
    State, COMMAND_STATE_PIPE, READ_CLEAR_PIPE, SELECTION_PIPE, SYNC_STATE_PIPE, TOGGLE_CWD_PIPE,
    WIDTH_PIPE,
};

// 目標の幅へ寄せるために撃つ相対リサイズの上限回数
//（docs/issues/issue-sidebar-width-persist-across-tabs.md）。zellij のリサイズは
// 端末幅の一定割合ずつ動く量子化された操作で、目標にぴったり乗る保証が無い。
// 乗らないまま撃ち続けると幅が振動するので回数で打ち切る
pub(crate) const WIDTH_MAX_ATTEMPTS: usize = 6;

// 目標との差がこの桁数以内なら到達として扱う。端末ウィンドウのリサイズでは
// 全タブが割合から桁数へ丸め直されるため、タブ間で1桁の食い違いが出うる。
// これを律儀に追うと1段階（端末幅の5%）まるごと動いて、かえって目標から離れる
const WIDTH_TOLERANCE: usize = 1;

// 幅の目標を教えてほしいという問い合わせ。桁数と同じ pipe に相乗りさせる
//（桁数は数値なので、数値にならない文字列とは取り違えない）
const WIDTH_REQUEST: &str = "?";

// 状態ダンプを教えてほしいという新入りからの問い合わせ。ダンプと同じ pipe に
// 相乗りさせる（ダンプの各行は必ずペインID＋タブ区切りで始まるので取り違えない）
const STATE_REQUEST: &str = "?";

// 指定インスタンスへ pipe を1本送る。宛先は必ずプラグインIDで指定する —
// URL指定（`with_plugin_url`）は配送ではなく**新しいプラグインの起動**になる
//（ファイル冒頭の経緯参照）
pub(crate) fn send_to_plugin(plugin_id: u32, pipe: &str, payload: String) {
    host::pipe_message_to_plugin(
        MessageToPlugin::new(pipe)
            .with_destination_plugin_id(plugin_id)
            .with_payload(payload),
    );
}

impl State {
    // fujin_toggle_cwd の受け口（docs/issues/issue-toggle-cwd-key.md）。
    // payload無し＝ユーザーのキー操作（各自反転）、"true"/"false"＝新入り
    // インスタンスへの現在値push（決定202608012141。そのままセット）
    pub(crate) fn handle_toggle_cwd_pipe(&mut self, payload: Option<&str>) -> bool {
        let requested = match payload.map(str::trim) {
            // 全インスタンスが同じ値から出発している前提で、各自が独立に反転
            // すれば権威なしで足並みが揃う。空文字も未設定と同じ扱い
            //（config.rs の正規化に揃える。CLI から叩くと payload が空で届きうる）
            None | Some("") => !self.show_cwd,
            // 明示セット — 反転にすると押し付けのたびに向きがずれる
            Some("true") => true,
            Some("false") => false,
            // 真偽値の受け口は広げない（決定202608080346。config.rs と同じ方針）。
            // 黙って false へ倒すと「cwd が消えた」結果だけが残る
            Some(raw) => {
                eprintln!("fujin: unparsable toggle_cwd payload: {}", raw);
                return false;
            }
        };
        if self.show_cwd == requested {
            return false;
        }
        self.show_cwd = requested;
        true
    }

    // fujin_selection の受け口。選択はインデックスではなく**ペインIDで**運ぶ
    //（決定202608012141）。インデックスは各インスタンスの selectable に依存し、一覧が
    // 古いインスタンスでは別の行を指してしまう
    pub(crate) fn handle_selection_pipe(&mut self, payload: Option<&str>) -> bool {
        payload
            .and_then(|p| p.trim().parse::<u32>().ok())
            .map(|target| self.select_pane_id(target))
            .unwrap_or(false)
    }

    // fujin_read の受け口。可視インスタンスが観測した既読クリアを取り込む
    pub(crate) fn handle_read_clear_pipe(&mut self, payload: Option<&str>) -> bool {
        let Some(raw) = payload else {
            return false;
        };
        let mut changed = false;
        for pane_id in raw.split(',').filter_map(|s| s.trim().parse::<u32>().ok()) {
            if let Some(agent) = self.agents.get_mut(&pane_id) {
                changed |= agent.mark_read();
            }
            // コマンド状態も同じ既読モデルに乗る（決定202608072218）。既読の猶予
            //（`awaiting_refocus`）は見ない — 送り手の可視インスタンスが
            // 猶予込みで判断した結果がここへ来る
            if let Some(info) = self.commands.get_mut(&pane_id) {
                changed |= info.force_read();
            }
        }
        changed
    }

    // 既知の兄弟インスタンス全員へ同じ payload を配る（決定202608012141）。
    // 配信ループはここ1本に集約し、pipe ごとに再実装しない
    pub(crate) fn broadcast_to_siblings(&self, pipe: &str, payload: &str) {
        for sibling in &self.known_siblings {
            send_to_plugin(*sibling, pipe, payload.to_string());
        }
    }

    // fujin_sync_state の受け口。payload が `?` のときは「状態を教えてほしい」と
    // いう新入りからの問い合わせで、それ以外はダンプ本体
    pub(crate) fn handle_sync_state_pipe(
        &mut self,
        payload: Option<&str>,
        source: &PipeSource,
    ) -> bool {
        let Some(raw) = payload else {
            return false;
        };
        if raw.trim() == STATE_REQUEST {
            if let PipeSource::Plugin(asker) = source {
                self.push_state_to(*asker);
            }
            return false;
        }
        self.apply_state_dump(raw)
    }

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
        // 受けると召喚（決定202608011644）が「own id/url unknown」で不発になる（実測）。
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
        // 幅だけは押し付けでは届かない（docs/issues/issue-sidebar-width-persist-across-tabs.md）。
        // 新しいタブを作ると新入りが前面に出て、既存インスタンスは非可視になり
        // PaneUpdate を受け取らない ＝ 新入りに気づけるのが新入り自身しかいない。
        // 状態と違って「まだ知らない側」から取りに行く
        if self.width_target.is_none() {
            self.broadcast_to_siblings(WIDTH_PIPE, WIDTH_REQUEST);
        }
        // エージェント状態も同じ理由で取りに行く（docs/issues/issue-tab-switch-agent-status-desync.md）。
        // 幅と違って「まだ知らない」を持ち物から判定できない — 自分が生まれた後に
        // 飛んできたフック通知で `agents` が1件埋まっているだけでも、それ以前から
        // 座っているペインの状態は欠けたままなので、空かどうかでは分岐できない。
        // 兄弟を初めて見つけた1回だけ要求する（以降の変化はフックが全員に届く）
        if !self.state_requested {
            self.state_requested = true;
            self.broadcast_to_siblings(SYNC_STATE_PIPE, STATE_REQUEST);
        }
        let dump = (!self.agents.is_empty()).then(|| self.state_dump());
        // コマンド状態も一緒に配る（決定202608072218）。導出できるのは PaneUpdate が届く
        // このインスタンスだけなので、新入りは押し付けられない限り一生知らない
        let commands = (!self.commands.is_empty()).then(|| self.command_dump());
        // マークも配る（決定202608080250）。タブをまたぐマークを許した以上、後からできた
        // タブのサイドバーにだけ印が出ないと「どれを選んだか」が食い違う
        let marked = !self.marked.is_empty();
        for id in newcomers {
            // cwd表示の現在値も配る（docs/issues/issue-toggle-cwd-key.md）。他と違って
            // 無条件に送る理由は push_show_cwd_to 側のコメントに書いてある
            self.push_show_cwd_to(id);
            // 幅の目標も配る（docs/issues/issue-sidebar-width-persist-across-tabs.md）。
            // 新しいタブはレイアウトの雛形の幅で開くので、これが無いと
            // 新規タブだけ幅が戻る
            self.push_width_to(id);
            if marked {
                self.push_marks_to(id);
            }
            if let Some(dump) = &dump {
                send_to_plugin(id, SYNC_STATE_PIPE, dump.clone());
            }
            if let Some(commands) = &commands {
                send_to_plugin(id, COMMAND_STATE_PIPE, commands.clone());
            }
        }
    }

    // 状態ダンプを要求してきた兄弟へ送り返す。空のときは送らない — 受け手は
    // 知らないペインを埋めるだけなので、空の payload は何も起こさない
    fn push_state_to(&self, plugin_id: u32) {
        if self.agents.is_empty() {
            return;
        }
        send_to_plugin(plugin_id, SYNC_STATE_PIPE, self.state_dump());
    }

    // 新入りへ cwd表示の現在値を伝える（docs/issues/issue-toggle-cwd-key.md）。
    //
    // 新入りは起動時のconfig既定値からしか出発できないので、実行中にトグルされて
    // いれば新入りだけ食い違う。他の配布物（状態ダンプ・マーク）と違って**中身が
    // 空かどうかで送るか決められない** — 既定値と同じ値であっても、相手には
    // 「トグルして既定へ戻した」のか「一度も触っていない」のか区別が付かないので、
    // 無条件に押し付ける
    fn push_show_cwd_to(&self, plugin_id: u32) {
        send_to_plugin(plugin_id, TOGGLE_CWD_PIPE, self.show_cwd.to_string());
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

    // 配られた状態ダンプを取り込む。戻り値は再描画するか。
    //
    // **自分が持っていないペインだけを埋める**（docs/issues/issue-tab-switch-agent-status-desync.md）。
    // 以前は「自分の `agents` が空のときだけ丸ごと取り込む」全か無かで、1件でも
    // 自前の登録があると欠けたまま直らなかった。逆に既にある登録を上書きしないのは、
    // フック通知は全インスタンスへ届く＝登録さえあれば中身は揃っているためで、
    // 上書きは既読クリア（決定202607302302）を取り消す方向にしか効かない
    pub(crate) fn apply_state_dump(&mut self, raw: &str) -> bool {
        let mut filled = 0usize;
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
            if self.agents.contains_key(&pane_id) {
                continue;
            }
            filled += 1;
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
        if filled == 0 {
            return false;
        }
        // 既に閉じたペインの状態が混ざらないようにする。**自分が可視のときだけ**
        // — 非可視インスタンスの `PaneManifest` は凍っている（`docs/dev/api-reference.md`
        // の配送表）ので、そこで間引くと配られたばかりの状態を「知らないペイン」
        // として捨ててしまう。取りこぼした掃除は次の `PaneUpdate` が済ませる
        if self.visible {
            self.prune_stale_agents();
        }
        // 食い違いを追うときの手がかりになるので残す
        eprintln!("fujin: synced {} agents from peer", filled);
        true
    }

    // --- サイドバー幅のタブ間追従（docs/issues/issue-sidebar-width-persist-across-tabs.md） ---
    //
    // zellij の `new_tab_template` はタブ生成時に複製されるだけの静的な雛形で、
    // 実行時のリサイズは書き戻されない。そこで「誰かがリサイズしたら、その桁数を
    // 兄弟インスタンスへ配り、各自が自分のペインを寄せる」で揃える。
    //
    // 絶対値で幅を指定するAPIは無い（`Resize` は Increase/Decrease だけ）ので、
    // 寄せるのは「自分の幅を見て1段階撃つ」を描画のたびに繰り返す帰還制御になる。
    // 非可視インスタンスにも pipe の受け口が true を返せば描画は回る（実測5ms。
    // `docs/dev/api-reference.md`）ので、背面のタブもその場で寄る。

    // 描画のたびに呼ぶ。前回の幅との差から「利用者にリサイズされたか」を見て、
    // 必要なら目標へ1段階寄せる。`viewport_cols` を更新する前に呼ぶこと
    pub(crate) fn reconcile_width(&mut self, cols: usize) {
        // 常駐サイドバー以外（プレビュー・召喚フローティング）は幅の権威にも
        // 寄せ先にもしない。どちらも自前の幅で開かれる別物
        if self.is_preview || self.summoned || cols == 0 {
            return;
        }
        let previous = self.viewport_cols;
        if let Some(fired) = self.width_adjusting {
            if previous == cols {
                // 撃った結果がまだ届いていない（別の理由の描画が挟まった。
                // 新規タブのロード直後は着地の 5ms の間にも描画が何度も来る）。
                // ここで撃ち足すと多重に飛び、あとから届く着地を利用者の操作と
                // 取り違えて配り直してしまう（実測でこの経路を踏んだ）。
                // 幅が動くまで何もせず待つ — 着地しないままなら（最小幅に
                // 当たっている等）、撃てない以上これ以上できることも無い
                return;
            }
            self.width_adjusting = None;
            let landed_as_fired = match fired {
                Resize::Increase => cols > previous,
                Resize::Decrease => cols < previous,
            };
            if landed_as_fired {
                // 自分が撃ったリサイズの着地。利用者の操作ではないので配り直さない
                self.settle_width_after_step(previous, cols);
            } else {
                // 撃った向きと逆に動いた ＝ 自分の着地ではありえない。
                // 着地を待つ間に利用者がリサイズした
                self.claim_width_authority(previous, cols);
            }
        } else if previous != 0 && previous != cols && self.width_target != Some(cols) {
            // 利用者のリサイズ。目標と同じ幅になっただけのときを除いてあるのは、
            // 自分が撃ったリサイズの結果が `width_adjusting` を倒したあとの
            // 描画で届く可能性があるため。それを利用者の操作と取り違えると、
            // 寄せ終えた側が権威を名乗って幅を配り直してしまう
            self.claim_width_authority(previous, cols);
        }
        self.apply_width_target(cols);
    }

    // 利用者のリサイズを観測した。ここが新しい権威になる
    fn claim_width_authority(&mut self, previous: usize, cols: usize) {
        self.width_target = Some(cols);
        self.width_attempts = 0;
        self.width_settled = false;
        eprintln!(
            "fujin: sidebar width {} -> {}, broadcasting",
            previous, cols
        );
        self.broadcast_to_siblings(WIDTH_PIPE, &cols.to_string());
    }

    // 1段階撃って動いたあとの後始末。目標を跨いだら、跨いだ両端のうち
    // 目標に近いほうで止める。刻みが目標に乗らないのは、ドラッグリサイズが
    // 1セル単位で動く（実測: mouse_handler が delta/viewport で percent を出す）
    // 一方、プラグインの resize は 5% 固定刻みしか撃てないため
    //（docs/issues/issue-sidebar-width-persist-across-tabs.md）
    fn settle_width_after_step(&mut self, previous: usize, cols: usize) {
        let Some(target) = self.width_target else {
            return;
        };
        let before = target as isize - previous as isize;
        let after = target as isize - cols as isize;
        if after == 0 || before.signum() == after.signum() {
            return;
        }
        if after.abs() <= before.abs() {
            // 跨いだ着地点のほうが目標に近い（か同距離）。ここが最寄り
            eprintln!(
                "fujin: sidebar width overshot {} -> {} (target {}), settling",
                previous, cols, target
            );
            self.width_settled = true;
        } else {
            // 跨ぐ前の幅のほうが近かった。settled を立てずに戻ると、apply が
            // 目標へ向けて撃つ ＝ 1段階戻る。戻した着地でまた跨ぐが、その
            // ときは目標との差が今より小さいので上の分岐で止まる
            eprintln!(
                "fujin: sidebar width overshot {} -> {} (target {}), stepping back",
                previous, cols, target
            );
        }
    }

    // 目標と食い違っていたら1段階だけ撃つ。結果は後続の描画で受け取る
    fn apply_width_target(&mut self, cols: usize) {
        let Some(target) = self.width_target else {
            return;
        };
        if target.abs_diff(cols) <= WIDTH_TOLERANCE {
            self.width_attempts = 0;
            self.width_settled = false;
            return;
        }
        if self.width_settled {
            return;
        }
        if self.width_attempts >= WIDTH_MAX_ATTEMPTS {
            eprintln!(
                "fujin: sidebar width gave up at {} (target {})",
                cols, target
            );
            self.width_settled = true;
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let resize = if cols < target {
            Resize::Increase
        } else {
            Resize::Decrease
        };
        // サイドバーは左端に置かれる（決定202607302257）ので、自分の幅を動かす境界は右側
        host::resize_pane_with_id(
            ResizeStrategy::new(resize, Some(Direction::Right)),
            PaneId::Plugin(own_id),
        );
        self.width_attempts += 1;
        self.width_adjusting = Some(resize);
    }

    // fujin_width の受け口。桁数をそのまま運ぶ。payload が `?` のときは
    // 「今の目標を教えてほしい」という新入りからの問い合わせ
    pub(crate) fn handle_width_pipe(&mut self, payload: Option<&str>, source: &PipeSource) -> bool {
        // CLI からの送信は payload 付きの1通目の後に payload 無しの2通目
        //（EOFマーカー）が届く。意味は無いので黙って捨てる
        //（docs/issues/issue-cli-pipe-testing-pitfalls.md）
        let Some(raw) = payload else {
            return false;
        };
        if raw.trim() == WIDTH_REQUEST {
            if let PipeSource::Plugin(asker) = source {
                self.push_width_to(*asker);
            }
            return false;
        }
        let Some(target) = raw.trim().parse::<usize>().ok().filter(|w| *w > 0) else {
            eprintln!("fujin: unparsable width payload: {:?}", raw);
            return false;
        };
        if self.width_target == Some(target) {
            return false;
        }
        self.width_target = Some(target);
        self.width_attempts = 0;
        self.width_settled = false;
        // 寄せるのは描画のとき（自分の幅が分かるのがそこだけ）なので、
        // 描き直させることが即ち寄せ直しの合図になる
        true
    }

    // 新入りへ現在の目標幅を伝える。cwd表示と同じく無条件に送る — 新入りは
    // レイアウトの雛形の幅で開くので、目標と一致しているかを自分では知らない
    fn push_width_to(&self, plugin_id: u32) {
        if let Some(target) = self.width_target {
            send_to_plugin(plugin_id, WIDTH_PIPE, target.to_string());
        }
    }

    // 選択を兄弟インスタンスへ配る（運搬形式は選択ペインID）。navモード中の
    // 移動は横取り中の1インスタンスにしか起きないため、これがないと
    // タブごとに違う行が光る
    pub(crate) fn broadcast_selection(&self) {
        let Some(entry) = self.selectable.get(self.selected) else {
            return;
        };
        self.broadcast_to_siblings(SELECTION_PIPE, &entry.pane_id.to_string());
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
        self.broadcast_to_siblings(READ_CLEAR_PIPE, &payload);
    }
}
