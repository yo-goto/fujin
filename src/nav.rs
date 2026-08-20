// navモード（決定202607310311）。
//
// zellij のモード（Ctrl+p の paneモード等）と同じ操作感を、ビルトインモードを
// 潰さずに実現する。config.kdl には入場キー1つだけを書き、モード内のキーは
// InterceptedKeyPress で横取りして解釈する。
//
// モード内のサブモードは、それぞれ専用モジュールが持つ（検索は `search`、
// 番号ジャンプは `jump`、終了操作は `termination`、トリアージは `triage`）。
// ここが持つのは入退場・選択の移動と、サブモードへの振り分けまで。

use zellij_tile::prelude::*;

use crate::host;
use crate::render::HelpRow;
use crate::{Selectable, State};

impl State {
    // fujin_up / fujin_down（直接キー方式・決定202607302258）の受け口。
    // 選択を動かすのは可視インスタンスだけ（決定202608012142）。全員が自前で動かすと、
    // 一覧が古いインスタンスでは境界判定とクランプの結果が違って選択がずれる
    pub(crate) fn handle_nav_step_pipe(&mut self, forward: bool) -> bool {
        if !self.refresh_focus() {
            return false;
        }
        if forward {
            self.select_next();
        } else {
            self.select_previous();
        }
        self.broadcast_selection();
        true
    }

    // fujin_go の受け口。副作用は可視インスタンスのみ実行（多重発行の防止・決定202608012142）
    pub(crate) fn handle_nav_go_pipe(&mut self) -> bool {
        if self.refresh_focus() {
            self.focus_selected();
        }
        false
    }

    // fujin_mode（navモードへの入場）の受け口。
    //
    // キーの横取りは権威インスタンス1つだけが行う。全員が intercept_key_presses()
    // を呼ぶと誰が受け取るか不定になる。refresh_focus() が入場直前の実フォーカスを
    // 取り込むので、enter_nav_mode() は最新のフォーカスを見て初期位置を決められる。
    //
    // なお召喚インスタンスにはこの pipe が届かない。キーバインドの `MessagePlugin` は
    // URL一致で配送されるが、召喚インスタンスは configuration に `summoned=true` を
    // 持つため一致しない（実測: 受信ログが一切出ない）。トグルは召喚役が担う（決定202608011644）
    pub(crate) fn handle_nav_mode_pipe(&mut self) -> bool {
        if self.refresh_focus() {
            if !self.nav_mode {
                self.enter_nav_mode();
                return true;
            }
            return false;
        }
        // ここへ来たインスタンスはフォーカス中のタブに居ない。そのタブに
        // fujin が1つも無ければ権威がどこにも立たず、pipe が届いても
        // 無反応になる。代表1つがフローティングで召喚して穴を埋める（決定202608011644）
        self.summon_floating_if_absent();
        false
    }

    pub(crate) fn enter_nav_mode(&mut self) {
        eprintln!("fujin: entering nav mode (summoned={})", self.summoned);
        self.nav_mode = true;
        if let Some(pane_id) = self.nav_entry_selection() {
            self.select_pane_id(pane_id);
        }
        self.broadcast_selection();
        self.park_focus();
        host::intercept_key_presses();
    }

    // 入場時に選択すべきペイン（要件: docs/requirements/req-focus-sync.md）。
    //
    // - 退場後にフォーカスが動いていた → 現在のフォーカスから始める。
    //   ユーザーが作業場所を変えた以上、古い探索位置を出すと
    //   「なぜここが選ばれているのか」と混乱を招く
    // - 動いていない（Escで抜けたまま実フォーカスに触れていない）→
    //   退場時の探索位置を復元する。「ちょっと確認して抜けたが、すぐ見たい」に応える
    //
    // Enterでジャンプして退場した場合は、ジャンプでフォーカスが選択行へ移るため
    // 前者の分岐を通り、結果としてジャンプ先が選ばれる（どちらでも同じ行になる）
    fn nav_entry_selection(&self) -> Option<u32> {
        let focused = self.focused_pane?;
        if self.focus_at_nav_exit == Some(focused) {
            self.selection_at_nav_exit.or(Some(focused))
        } else {
            Some(focused)
        }
    }

    pub(crate) fn exit_nav_mode(&mut self) {
        self.nav_mode = false;
        // ヘルプオーバーレイ・サブモード・プレビューは全て navモードの内側の
        // 表示なので一緒に畳む。開いたまま退場するとキーが届かず戻れなくなる。
        // 入力途中のバッファも、確認を経ていない終了操作も持ち越さない
        self.help_overlay = false;
        // 次の入場で「退場後にフォーカスが動いたか」を判定するために控える
        //（要件: focus-sync）
        self.focus_at_nav_exit = self.focused_pane;
        self.selection_at_nav_exit = self.selectable.get(self.selected).map(|e| e.pane_id);
        self.search = None;
        self.triage = None;
        self.jump = None;
        self.termination = None;
        // プレビューを畳むのは**預かったフォーカスを返すより前**（決定202608082045）。
        // 順が逆だと、返した先のフォーカスがプレビューの後始末で持って行かれかねない
        self.close_preview();
        // フォーカスの返却（決定202608072359）は召喚インスタンスの自死（下）より前に置く —
        // 自分を閉じたあとではホストコマンドが届くか分からない
        self.release_parked_focus(true);
        host::clear_key_presses_intercepts();
        // 召喚インスタンスは用が済んだら自分で退場する（決定202608011644）。残すと作業
        // ペインに重なり続ける。次の入場でまた呼べばよい（召喚から入場まで実測16ms）
        if self.summoned {
            if let Some(own_id) = self.own_plugin_id {
                host::close_plugin_pane(own_id);
            }
        }
    }

    // ジャンプを伴わない退場（Esc / q / 未定義キー、および navモード中に実フォーカスが
    // 動いたとき）。navモード外のハイライトは常に実フォーカスと一致するので、
    // 探索で動かした選択はここで戻す（要件: focus-sync）。探索位置そのものは
    // exit_nav_mode() が控えていて、フォーカスが動かないまま入り直せば復元される
    pub(crate) fn leave_nav_mode(&mut self) {
        self.exit_nav_mode();
        if let Some(focused) = self.focused_pane {
            if self.select_pane_id(focused) {
                self.broadcast_selection();
            }
        }
    }

    // ペインIDで選択を移す。戻り値は選択が動いたか。
    // インデックスではなくペインIDを入口にするのは決定202608012141と同じ理由で、
    // 一覧が古いインスタンスでも同じ行を指せるようにするため
    pub(crate) fn select_pane_id(&mut self, pane_id: u32) -> bool {
        let Some(index) = self.selectable.iter().position(|e| e.pane_id == pane_id) else {
            return false;
        };
        let changed = self.selected != index;
        self.selected = index;
        changed
    }

    // 選択を1行下へ。末尾で止まる（pipe とキー操作の両方から使う）
    pub(crate) fn select_next(&mut self) {
        if self.selected + 1 < self.selectable.len() {
            self.selected += 1;
        }
    }

    // 選択を1行上へ。先頭で止まる
    pub(crate) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    // モード中のキー解釈。戻り値は再描画するか
    pub(crate) fn handle_nav_key(&mut self, key: KeyWithModifier) -> bool {
        // ヘルプオーバーレイ表示中はどのキーでも閉じるだけで、キーそのものは
        // 操作として解釈しない（要件: nav-mode-hints）。安全弁（決定202607310311）より
        // 手前に置くのは、修飾キー付きでも「閉じる」で済ませてnavモードを
        // 継続させるため — 閲覧を終わらせただけで退場するのは筋が通らない
        if self.help_overlay {
            self.help_overlay = false;
            return true;
        }
        // `?` はヘルプオーバーレイを開く。検索サブモードへの振り分けより手前に
        // 置く — 検索中の印字可能文字はクエリになるので、後ろに置くと `?` が
        // クエリへ入ってヘルプを呼べなくなる。
        // **例外は検索サブモードの編集状態だけ**（決定202608131200）。そこでは `?` を
        // クエリに打てることを優先し、ヘルプは操作状態（Esc で移る）から開く
        if key.bare_key == BareKey::Char('?')
            && !has_hard_modifier(&key)
            && !self.search_is_editing()
        {
            self.help_overlay = true;
            return true;
        }
        // 検索サブモード中は専用ハンドラへ。下の修飾キー判定より手前に
        // 置くこと — 検索側は Shift+Tab（修飾付き）を通す必要がある
        if self.search.is_some() {
            return self.handle_search_key(key);
        }
        // トリアージモードも同様に専用ハンドラへ（要件: triage-mode）
        if self.triage.is_some() {
            return self.handle_triage_key(key);
        }
        // 番号ジャンプサブモードも同様（要件: pane-number-jump）
        if self.jump.is_some() {
            return self.handle_jump_key(key);
        }
        // 終了操作サブモードも同様（要件: pane-close-kill）。フッターの中は
        // 専用のキー空間で、navモード本体の `k` とはここで分かれる
        if self.termination.is_some() {
            return self.handle_termination_key(key);
        }
        // Shift は素通し（`G` が Shift付きで来る端末があるため）。
        // Ctrl/Alt/Super 付きは未定義なので抜けて安全側に倒す
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        // 1文字ショートカットは頭文字（t=triage, n=number, d=delete, m=mark,
        // p=preview, r=read）で、いずれも navモード内で未使用だったキー
        match key.bare_key {
            // 検索サブモードへ（要件: docs/requirements/req-search-explorer.md）
            BareKey::Char('/') => self.enter_search(),
            // トリアージモードへ（要件: docs/requirements/req-triage-mode.md）
            BareKey::Char('t') => self.enter_triage(),
            // 番号ジャンプサブモードへ（要件: docs/requirements/req-pane-number-jump.md）。
            // かつての 1-9 直行ジャンプはここへ一本化して削除した（決定202608070342）。
            // navモード最上位の数字は未定義キー＝安全弁の扱い
            BareKey::Char('n') => self.enter_jump(),
            // 終了操作サブモードへ（決定202608080140。要件: docs/requirements/req-pane-close-kill.md）。
            // close/kill/kill→close を独立キーにすると押し間違いのリスクが高いので、
            // 入場キー1つ＋確認プロンプトのミニフローに畳んである
            BareKey::Char('d') => self.enter_termination(),
            // マークのトグルと全解除（決定202608080250）。専用サブモードは作らない —
            // トグルだけの軽い操作なので、一覧の上で直接積み上げる
            BareKey::Char('m') => self.toggle_mark(),
            BareKey::Char('M') => self.clear_marks(),
            // プレビューのトグルと、プレビュー中の既読化（決定202608082045。マークと同じく
            // サブモード無しの横断的操作）。`r` はプレビューがオフの間、未定義キー
            // として安全弁に倒れる（判定は mark_preview_read の中）
            BareKey::Char('p') => self.toggle_preview(),
            BareKey::Char('r') => self.mark_preview_read(),
            BareKey::Down | BareKey::Tab | BareKey::Char('j') => self.select_next(),
            BareKey::Up | BareKey::Char('k') => self.select_previous(),
            BareKey::Char('g') => self.selected = 0,
            BareKey::Char('G') => {
                self.selected = self.selectable.len().saturating_sub(1);
            }
            BareKey::Enter | BareKey::Char(' ') | BareKey::Char('l') => {
                // フォーカス移動でタブが変わりうるので、先に横取りを解除する
                self.exit_nav_mode();
                self.focus_selected();
            }
            // Esc / q は明示的な退場。それ以外の未定義キーでも抜ける:
            // 万一プラグインが応答不能になってもキー入力が取り残されないため
            _ => self.leave_nav_mode(),
        }
        // 横取り中の移動は自分にしか起きないので、都度配る
        self.broadcast_selection();
        // プレビューがオンなら表示を光っている行へ追従させる（決定202608082045）。
        // 退場した後は preview を畳んであるので何も起きない
        self.refresh_preview();
        true
    }

    // 行クリックでのフォーカス移動
    //（要件: docs/requirements/req-click-to-focus.md）。戻り値は再描画するか。
    //
    // 引数の `line` は描画時のy座標そのもの（zellij は isize で渡してくる）。
    // 行→ペインの対応は render 側のレイアウトから引く。
    // サイドバーは set_selectable(false) のままだが、マウスイベントの配送は
    // selectable にもフォーカスにも縛られない（実測。決定202607302258・16と衝突しない）
    pub(crate) fn handle_click(&mut self, line: isize) -> bool {
        // 負の行はサイドバーの外
        let Ok(row) = usize::try_from(line) else {
            return false;
        };
        // タブ見出し・ヘッダ・一覧の外側のクリックは何も起こさない
        let Some(pane_id) = self.pane_at_row(row) else {
            return false;
        };
        // navモード中にマウスが届いた場合も Enter と同じ扱いにする（v1 の要件は
        // navモード外の行クリックのみだが、横取りを残したままフォーカスだけ動かすと
        // 移動先で j/k を食われ続けるため、取り残しを作らない側に倒す）
        if self.nav_mode {
            self.exit_nav_mode();
        }
        self.select_pane_id(pane_id);
        self.broadcast_selection();
        self.focus_selected();
        true
    }

    pub(crate) fn focus_selected(&self) {
        if let Some(entry) = self.selectable.get(self.selected) {
            // 第2引数 should_float_if_hidden はターゲットに合わせて切り替える。
            // false のままだと**フローティング層が隠れているタブのフローティング
            // ペインにジャンプできない**（タブ切り替えすら起きず無反応）。
            // かといって常に true にすると、今度は**フローティング層を表示中に
            // タイルペインへ戻れなくなる**。どちらも実測で確認済み
            host::focus_pane_with_id(PaneId::Terminal(entry.pane_id), entry.is_floating, false);
        }
    }

    // ヘルプオーバーレイに出すキー一覧（要件: nav-mode-hints）。
    // 表示中のモードで内容を出し分ける。文言は英語で統一する。
    //
    // 整形と配色は render 側の責務なので、ここは「どのキーに何が割り当たって
    // いるか」だけを持つ。矢印など幅の曖昧な文字は使わない — サイドバーの
    // 幅計算が文字数ベース（v1）なので、キー列の位置がずれるため
    pub(crate) fn help_lines(&self) -> &'static [HelpRow] {
        use HelpRow::{Blank, Entry, Section};
        if self.triage.is_some() {
            self.triage_help_lines()
        } else if self.termination.is_some() {
            self.termination_help_lines()
        } else if self.jump.is_some() {
            &[
                Section("keys"),
                Blank,
                Entry("0-9", "narrow & jump"),
                Entry("backspace", "delete digit"),
                Entry("esc", "back to tree"),
                Entry("?", "this help"),
            ]
        } else if self.search.is_some() {
            // 開けるのは操作状態からだけ（編集状態の `?` はクエリの文字。決定202608131200）
            // なので操作状態のキーを先に置き、`i` で戻る先の編集状態のキーを続ける
            &[
                Section("keys"),
                Blank,
                Entry("j k", "move cursor"),
                Entry("shift+tab", "move back"),
                Entry("i", "edit query"),
                Entry("type", "filter panes"),
                Entry("backspace", "delete char"),
                // クエリ入力と両立しないので、マークとプレビューは Alt付き
                //（決定202608080250・決定202608082045）
                Entry("alt+m", "mark"),
                Entry("alt+p", "preview"),
                Entry("enter", "jump & exit"),
                Entry("esc", "cancel search"),
                Entry("?", "this help"),
            ]
        } else if self.preview.is_some() {
            // プレビュー中だけ既読化キーが増える（決定202608082045）。オフの間は
            // 未定義キー扱いなので、押せないキーをヘルプに残さない
            &[
                Section("keys"),
                Blank,
                Entry("j k", "move"),
                Entry("g G", "top / bottom"),
                Entry("enter", "jump & exit"),
                Entry("/", "search"),
                Entry("t", "triage"),
                Entry("n", "number jump"),
                Entry("m M", "mark / clear all"),
                Entry("p", "preview off"),
                Entry("r", "mark read"),
                Entry("d", "terminate pane"),
                Entry("?", "this help"),
                Entry("esc", "exit"),
            ]
        } else {
            &[
                Section("keys"),
                Blank,
                Entry("j k", "move"),
                Entry("g G", "top / bottom"),
                Entry("enter", "jump & exit"),
                Entry("/", "search"),
                Entry("t", "triage"),
                Entry("n", "number jump"),
                Entry("m M", "mark / clear all"),
                Entry("p", "preview"),
                Entry("d", "terminate pane"),
                Entry("?", "this help"),
                Entry("esc", "exit"),
            ]
        }
    }

    // 選択対象（ターミナルペイン）のフラットリストをタブ順で再構築
    pub(crate) fn rebuild_selectable(&mut self) {
        self.selectable.clear();
        let Some(manifest) = &self.panes else {
            return;
        };
        let mut positions: Vec<usize> = manifest.panes.keys().copied().collect();
        positions.sort();
        for position in positions {
            if let Some(panes) = manifest.panes.get(&position) {
                for pane in panes {
                    if pane.is_plugin || pane.is_suppressed {
                        continue;
                    }
                    self.selectable.push(Selectable {
                        tab_position: position,
                        pane_id: pane.id,
                        // ペイン名が空のコマンドペインはコマンド文字列を名前に
                        // 使う（決定202608072218）。ここで畳んでおくと、検索・切り詰め・
                        // ハイライトの経路が普通のペイン名と同じままで済む
                        title: crate::command::fallback_title(pane),
                        is_floating: pane.is_floating,
                    });
                }
            }
        }
        if self.selected >= self.selectable.len() {
            self.selected = self.selectable.len().saturating_sub(1);
        }
        // 閉じられたペインのマークもここで落とす（決定202608080250。選択のクランプと同じ
        // 場所で済ませる）。一覧を組めなかった場合は上で return しているので、
        // 一覧が空＝本当にペインが無いときにしか消えない
        self.prune_marks();
        // 検索中にペインが増減したら絞り込みを引き直す。
        // 古い hits のままだと閉じたペインが結果に残り続ける
        if self.search.is_some() {
            self.refilter();
        }
        // トリアージ一覧はここで引き直さない。毎フレーム組み直すうえ、
        // カーソルも参照のたびに畳んでいる（triage::triage_cursor）
    }
}

// Shift 以外の修飾キー（Ctrl / Alt / Super）が付いているか。
// 安全弁（決定202607310311）の判定条件で、Shift だけは印字可能文字の一部として素通しする
pub(crate) fn has_hard_modifier(key: &KeyWithModifier) -> bool {
    key.key_modifiers.iter().any(|m| *m != KeyModifier::Shift)
}
