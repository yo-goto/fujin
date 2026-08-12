// navモード（決定12）と検索サブモード（要件: docs/requirements/search-explorer/）。
//
// zellij のモード（Ctrl+p の paneモード等）と同じ操作感を、ビルトインモードを
// 潰さずに実現する。config.kdl には入場キー1つだけを書き、モード内のキーは
// InterceptedKeyPress で横取りして解釈する。

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::render::HelpRow;
use crate::search::{match_pane, Hit};
use crate::{ParkedFocus, Selectable, State};

// 番号ジャンプサブモード（navモード内の `n`、決定29）のローカルUI状態。
// 検索サブモードと同じく権威インスタンスにしか発生しないため、
// 兄弟インスタンスへは配らない（決定13の範囲外）
#[derive(Debug, Default)]
pub(crate) struct JumpState {
    // 番号入力バッファ。数字が入るたびに通し番号へ前方一致で照合し、
    // 候補が1件になった時点でジャンプ、0件になったら空へ戻す（決定29）
    pub(crate) buffer: String,
}

// 検索サブモード（navモード内の `/`）のローカルUI状態。
// 権威インスタンスにしか発生しないため、兄弟への同期は不要（決定13の範囲外）
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) query: String,
    // ペインID -> ヒット情報。ツリー順は selectable 側が持つので順序は持たない
    pub(crate) hits: BTreeMap<u32, Hit>,
    // 絞り込み結果内のカーソル（ペインID）。インデックスで持つと
    // rebuild_selectable() をまたいだときに別の行を指す（決定13が禁じた罠のローカル版）
    pub(crate) cursor: Option<u32>,
    // 検索サブモードに入る前の選択（Esc で戻すため）
    pub(crate) saved: Option<u32>,
}

impl State {
    // fujin_up / fujin_down（直接キー方式・決定6）の受け口。
    // 選択を動かすのは可視インスタンスだけ（決定14）。全員が自前で動かすと、
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

    // fujin_go の受け口。副作用は可視インスタンスのみ実行（多重発行の防止・決定14）
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
    // 持つため一致しない（実測: 受信ログが一切出ない）。トグルは召喚役が担う（決定16）
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
        // 無反応になる。代表1つがフローティングで召喚して穴を埋める（決定16）
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
        intercept_key_presses();
    }

    // navモード中だけ、実フォーカスをサイドバー自身へ預かる（決定34）。
    //
    // フォーカス枠の色だけを消すAPIは無いが、**枠はセッション内で1枚しか点かない**
    //（実測）ので、フォーカスをサイドバーへ移せば作業ペインの枠は非フォーカス色に
    // 戻り、サイドバーのハイライトと二重に「ここが操作対象」を主張しなくなる。
    // 召喚インスタンス（決定16）は最初から自分がフォーカスを持っているので、
    // これは常駐サイドバーを召喚と同じ状態に揃える操作でもある
    fn park_focus(&mut self) {
        // 既に預かっている（入場のやり直し）なら二重に動かさない
        if self.summoned || self.focus_parked.is_some() {
            return;
        }
        // 実フォーカスがプラグインペイン側にあるなら、作業ペインに枠は
        // 点いていない（上記の1枚だけの性質）。預かる理由が無い
        if !self.focus_on_terminal {
            return;
        }
        let (Some(pane_id), Some(own_id)) = (self.focused_pane, self.own_plugin_id) else {
            return;
        };
        let is_floating = self.pane_is_floating(pane_id);
        // unselectable なペインはフォーカスできない（実測。api-reference.md）ので、
        // 預かる間だけ selectable に戻す。navモード中は全キーを横取りしているため
        // 巡回でサイドバーへ入り込む余地は無く、決定6の意図は保たれる
        set_selectable(true);
        focus_plugin_pane(own_id, false, false);
        self.focus_parked = Some(ParkedFocus {
            pane_id,
            is_floating,
            confirmed: false,
        });
    }

    // 預かったフォーカスを手放す（決定34）。`refocus` が真なら作業ペインへ返す。
    //
    // 退場の2系統（exit_nav_mode / leave_nav_mode）と `Event::BeforeClose` の
    // どこを通っても必ず手放すため、「預かった相手」を覚えておいて冪等に戻す。
    // ジャンプ経路も一度は作業ペインへ返す — 呼び出し側の `focus_selected()` が
    // すぐ上書きするが、フローティング層の出し入れを含む移動の起点を
    // navモードに入る前と同じ状態に揃えられる
    pub(crate) fn release_parked_focus(&mut self, refocus: bool) {
        let Some(parked) = self.focus_parked.take() else {
            return;
        };
        if refocus {
            if let Some((pane_id, is_floating)) = self.refocus_target(parked) {
                focus_pane_with_id(PaneId::Terminal(pane_id), is_floating, false);
            }
        }
        // 決定6へ戻す。**フォーカスを返したあとに** unselectable にすること —
        // 逆順だと自分にフォーカスが残ったまま巡回対象から外れる
        set_selectable(false);
    }

    // フォーカスの返し先。原則は預かった当のペインだが、**navモード中に
    // 閉じられていたら選択行のペインへ返す**（決定34）。unselectable な自分に
    // フォーカスが残ると、横取りも解いた後なのでキーの行き先が無くなる
    pub(crate) fn refocus_target(&self, parked: ParkedFocus) -> Option<(u32, bool)> {
        if self.selectable.iter().any(|e| e.pane_id == parked.pane_id) {
            return Some((parked.pane_id, parked.is_floating));
        }
        self.selectable
            .get(self.selected)
            .map(|entry| (entry.pane_id, entry.is_floating))
    }

    // 入場時に選択すべきペイン（要件: docs/requirements/focus-sync/）。
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
        // プレビューを畳むのは**預かったフォーカスを返すより前**（決定42）。
        // 順が逆だと、返した先のフォーカスがプレビューの後始末で持って行かれかねない
        self.close_preview();
        // フォーカスの返却（決定34）は召喚インスタンスの自死（下）より前に置く —
        // 自分を閉じたあとではホストコマンドが届くか分からない
        self.release_parked_focus(true);
        clear_key_presses_intercepts();
        // 召喚インスタンスは用が済んだら自分で退場する（決定16）。残すと作業
        // ペインに重なり続ける。次の入場でまた呼べばよい（召喚から入場まで実測16ms）
        if self.summoned {
            if let Some(own_id) = self.own_plugin_id {
                close_plugin_pane(own_id);
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
    // インデックスではなくペインIDを入口にするのは決定13と同じ理由で、
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
        // 操作として解釈しない（要件: nav-mode-hints）。安全弁（決定12）より
        // 手前に置くのは、修飾キー付きでも「閉じる」で済ませてnavモードを
        // 継続させるため — 閲覧を終わらせただけで退場するのは筋が通らない
        if self.help_overlay {
            self.help_overlay = false;
            return true;
        }
        // `?` はヘルプオーバーレイを開く。検索サブモードへの振り分けより手前に
        // 置く — 検索中の印字可能文字はクエリになるので、後ろに置くと `?` が
        // クエリへ入ってヘルプを呼べなくなる（検索サブモード中も `?` で開ける
        // ことが要件。代償としてクエリに `?` は打てない）
        if key.bare_key == BareKey::Char('?') && !has_hard_modifier(&key) {
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
        // 1文字ショートカットは頭文字（p=priority, n=number, d=delete, m=mark,
        // v=view, r=read）で、いずれも navモード内で未使用だったキー
        match key.bare_key {
            // 検索サブモードへ（要件: docs/requirements/search-explorer/）
            BareKey::Char('/') => self.enter_search(),
            // トリアージモードへ（要件: docs/requirements/triage-mode/）
            BareKey::Char('p') => self.enter_triage(),
            // 番号ジャンプサブモードへ（要件: docs/requirements/pane-number-jump/）。
            // かつての 1-9 直行ジャンプはここへ一本化して削除した（決定29）。
            // navモード最上位の数字は未定義キー＝安全弁の扱い
            BareKey::Char('n') => self.enter_jump(),
            // 終了操作サブモードへ（決定35。要件: docs/requirements/pane-close-kill/）。
            // close/kill/kill→close を独立キーにすると押し間違いのリスクが高いので、
            // 入場キー1つ＋確認プロンプトのミニフローに畳んである
            BareKey::Char('d') => self.enter_termination(),
            // マークのトグルと全解除（決定39）。専用サブモードは作らない —
            // トグルだけの軽い操作なので、一覧の上で直接積み上げる
            BareKey::Char('m') => self.toggle_mark(),
            BareKey::Char('M') => self.clear_marks(),
            // プレビューのトグルと、プレビュー中の既読化（決定42。マークと同じく
            // サブモード無しの横断的操作）。`r` はプレビューがオフの間、未定義キー
            // として安全弁に倒れる（判定は mark_preview_read の中）
            BareKey::Char('v') => self.toggle_preview(),
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
        // プレビューがオンなら表示を光っている行へ追従させる（決定42）。
        // 退場した後は preview を畳んであるので何も起きない
        self.refresh_preview();
        true
    }

    // 検索サブモード中のキー解釈。navモードの安全弁（決定12）を検索サブモード用に
    // 引き直したもの。印字可能文字はクエリに使うため、1文字ショートカットは全て無効になる
    fn handle_search_key(&mut self, key: KeyWithModifier) -> bool {
        // マーク（決定39）とプレビュー（決定42）は絞り込み結果の上でも使えるよう、
        // 安全弁（下の has_hard_modifier）の例外として Alt付きで通す。
        // マーク全解除（Esc で戻ってから押せばよい）と既読化（取り消せない操作）は
        // この例外を広げない
        if key.bare_key == BareKey::Char('m') && key.key_modifiers.contains(&KeyModifier::Alt) {
            self.toggle_mark();
            return true;
        }
        if key.bare_key == BareKey::Char('v') && key.key_modifiers.contains(&KeyModifier::Alt) {
            self.toggle_preview();
            return true;
        }
        // Shift だけは素通し（Shift付き印字可能文字と Shift+Tab のため）。
        // それ以外の修飾キーは安全弁 — 検索サブモードだけでなく navモードごと抜ける
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        let shifted = key.key_modifiers.contains(&KeyModifier::Shift);
        match key.bare_key {
            // Esc は二段階の1段目: クエリを破棄して navモードへ戻るだけ。
            // exit_nav_mode() を呼んではいけない — 召喚インスタンスなら
            // 検索の取り消しでサイドバーごと閉じてしまう（決定16）
            BareKey::Esc => self.exit_search(),
            BareKey::Enter => self.confirm_search(),
            BareKey::Backspace => {
                if let Some(search) = &mut self.search {
                    search.query.pop();
                }
                self.refilter();
            }
            BareKey::Up => self.move_search_cursor(false),
            BareKey::Down => self.move_search_cursor(true),
            BareKey::Tab => self.move_search_cursor(!shifted),
            BareKey::Char(c) => {
                if let Some(search) = &mut self.search {
                    search.query.push(c);
                }
                self.refilter();
            }
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        // 検索中の移動・入力では broadcast_selection() を呼ばない。
        // Esc で「検索前の位置に戻す」以上、途中経過を配ると兄弟だけが
        // 取り消せない位置に取り残される（決定13）。配るのは確定時
        //（confirm_search）だけ
        //
        // プレビューは配布ではなく自分の表示なので、絞り込みのカーソルにも
        // そのまま追従させる（決定42）
        self.refresh_preview();
        true
    }

    // 番号ジャンプサブモード（要件: docs/requirements/pane-number-jump/）。
    //
    // 選択対象の全ペインに全タブ貫通の通し番号を振り、番号の入力でジャンプする。
    // 曖昧性解消方式（vimiumのリンクヒントに近い）: 数字を1つ入力するたびに
    // 前方一致で候補を絞り込み、1件に確定した時点で即ジャンプする（決定29）
    fn enter_jump(&mut self) {
        self.jump = Some(JumpState::default());
    }

    // 通し番号の桁数（番号列の幅）。総数の桁数に固定し、全番号をゼロ埋めで
    // 揃える（決定29）。桁数を固定すると番号どうしが互いの前方一致にならず
    //（prefix-free）、「1 を打ったが 10 があるので確定できない」という
    // 行き止まりが構造的に起きない
    pub(crate) fn pane_number_width(&self) -> usize {
        self.selectable.len().to_string().len()
    }

    // 選択対象 flat_index 番目の行に振る通し番号の表示（ゼロ埋め、1始まり）
    pub(crate) fn pane_number(&self, flat_index: usize) -> String {
        format!(
            "{:0width$}",
            flat_index + 1,
            width = self.pane_number_width()
        )
    }

    // 番号ジャンプサブモード中、この行の番号列に出すセル。
    // 返り値は (通し番号の表示, 番号入力バッファに前方一致して候補に残っているか)。
    // サブモード外は None ＝ 番号列そのものを出さない（決定29: 平常時の幅配分を崩さない）
    pub(crate) fn jump_number(&self, flat_index: usize) -> Option<(String, bool)> {
        let jump = self.jump.as_ref()?;
        let number = self.pane_number(flat_index);
        let matches = number.starts_with(&jump.buffer);
        Some((number, matches))
    }

    // 番号ジャンプサブモード中のキー解釈。数字だけを受け、それ以外は
    // navモード本体と同じ安全弁（決定12）に倒す
    fn handle_jump_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift 以外の修飾キーは安全弁 — サブモードだけでなく navモードごと抜ける
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        match key.bare_key {
            // Esc はサブモードだけ抜けて navモードに留まる（検索・トリアージと
            // 同じパターン）。exit_nav_mode() を呼んではいけない — 召喚
            // インスタンスなら番号入力の取り消しでサイドバーごと閉じてしまう（決定16）
            BareKey::Esc => self.jump = None,
            BareKey::Backspace => {
                if let Some(jump) = &mut self.jump {
                    jump.buffer.pop();
                }
            }
            BareKey::Char(c @ '0'..='9') => self.push_jump_digit(c),
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        // 番号入力の途中経過は兄弟インスタンスへ配らない（決定29。検索サブモードと
        // 同じ扱い）。確定時のジャンプは push_jump_digit 側で配る
        //
        // プレビューも更新しない（決定42）。候補を絞っているあいだは対象ペインが
        // 定まらないので、オンのまま入ってきた場合は直前の表示を保つ
        //（`refresh_preview` が番号ジャンプサブモード中は何もしない）
        true
    }

    // 数字を1つ足して候補を引き直す。前方一致の候補が1件になったら即ジャンプ、
    // 0件になったらバッファを空に戻して次の数字からやり直す（決定29。
    // Backspace での訂正を強制しないための救済）
    fn push_jump_digit(&mut self, digit: char) {
        let Some(jump) = &mut self.jump else {
            return;
        };
        jump.buffer.push(digit);
        let buffer = jump.buffer.clone();
        let (first, ambiguous) = {
            let mut candidates = (0..self.selectable.len())
                .filter(|&index| self.pane_number(index).starts_with(&buffer));
            let first = candidates.next();
            (first, candidates.next().is_some())
        };
        match first {
            // 0件: 存在しない番号。その場でリセットして打ち直させる
            None => {
                if let Some(jump) = &mut self.jump {
                    jump.buffer.clear();
                }
            }
            // 1件: 確定。既存のジャンプと同じ手順で navモードごと抜ける
            //（検索サブモードの confirm_search と同じ順序）
            Some(index) if !ambiguous => {
                self.selected = index;
                self.exit_nav_mode();
                self.broadcast_selection(); // 確定時だけ配る（決定13）
                self.focus_selected();
            }
            // 2件以上: まだ曖昧。次の数字を待つ
            Some(_) => {}
        }
    }

    fn enter_search(&mut self) {
        let saved = self.selectable.get(self.selected).map(|e| e.pane_id);
        self.search = Some(SearchState {
            cursor: saved,
            saved,
            ..SearchState::default()
        });
        self.refilter();
    }

    // Esc の1段目。クエリを破棄し、検索サブモードに入る前の選択へ戻して
    // navモードに留まる
    fn exit_search(&mut self) {
        let Some(search) = self.search.take() else {
            return;
        };
        if let Some(saved) = search.saved {
            self.select_pane_id(saved);
        }
    }

    fn confirm_search(&mut self) {
        // 0件ヒット時の Enter は何もしない（検索サブモードに留まる）
        let Some(cursor) = self.search.as_ref().and_then(|s| s.cursor) else {
            return;
        };
        let Some(index) = self.selectable.iter().position(|e| e.pane_id == cursor) else {
            return;
        };
        self.selected = index;
        // フォーカス移動でタブが変わりうるので、先に横取りを解除する
        //（navモードの Enter と同じ順序。search も一緒に破棄される）
        self.exit_nav_mode();
        self.broadcast_selection(); // 確定時だけ配る（決定13）
        self.focus_selected();
    }

    // 一括で届くテキスト入力（貼り付けと、**IMEの変換確定**）の受け口。
    //
    // 複数文字が一度に来る入力は `InterceptedKeyPress` ではなく `PastedText` に
    // 分かれる（zellij クライアントの入力ハンドラが、まとまった文字列を
    // `InputEvent::Paste` として解釈するため）。購読していないと、変換で確定した
    // 文字列が丸ごと消えたように見える（docs/issues/ime-input-support.md）。
    //
    // navモードの安全弁（決定12）はここには効かせない — 未定義の**キー**で抜ける
    // 仕組みであって、入力欄の外に落ちたテキストは操作ではないので黙って捨てる
    pub(crate) fn handle_pasted_text(&mut self, text: &str) -> bool {
        // ヘルプオーバーレイ中は入力欄が画面に無い（キーも「閉じる」にしか
        // 使われない）。キーと扱いを揃え、見えないクエリへは流さない
        if self.help_overlay {
            return false;
        }
        let Some(search) = &mut self.search else {
            return false;
        };
        // クエリは1行。改行やタブが混ざったペーストでも欄を壊さない
        let before = search.query.len();
        search
            .query
            .extend(text.chars().filter(|c| !c.is_control()));
        if search.query.len() == before {
            return false;
        }
        self.refilter();
        self.refresh_preview();
        true
    }

    // 絞り込みの再計算。クエリの変化と一覧の作り直し（rebuild_selectable）の
    // 両方から呼ばれる。ペインの増減で古い結果のまま表示しないため
    fn refilter(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        let mut hits: BTreeMap<u32, Hit> = BTreeMap::new();
        for entry in &self.selectable {
            let tab_name = self.tab_name(entry.tab_position);
            let cwd = self.pane_cwds.get(&entry.pane_id).map(String::as_str);
            if let Some(hit) = match_pane(&search.query, &entry.title, tab_name, cwd) {
                hits.insert(entry.pane_id, hit);
            }
        }
        // カーソルの追従はペインIDで行う。絞り込み結果に残っていれば維持し、
        // 消えていればツリー順の先頭ヒットへ寄せる。0件なら None
        let cursor = search
            .cursor
            .filter(|id| hits.contains_key(id))
            .or_else(|| {
                self.selectable
                    .iter()
                    .map(|e| e.pane_id)
                    .find(|id| hits.contains_key(id))
            });
        // 組み立ては不変借用で済ませ、最後にまとめて書き戻す
        if let Some(search) = &mut self.search {
            search.hits = hits;
            search.cursor = cursor;
        }
    }

    // 絞り込み結果内のカーソル移動。ツリー順で前後へ動かし、端で止まる
    fn move_search_cursor(&mut self, forward: bool) {
        let Some(search) = &self.search else {
            return;
        };
        let hit_ids: Vec<u32> = self
            .selectable
            .iter()
            .map(|e| e.pane_id)
            .filter(|id| search.hits.contains_key(id))
            .collect();
        if hit_ids.is_empty() {
            return;
        }
        let current = search
            .cursor
            .and_then(|c| hit_ids.iter().position(|&id| id == c));
        let next = match current {
            Some(i) if forward => (i + 1).min(hit_ids.len() - 1),
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        if let Some(search) = &mut self.search {
            search.cursor = Some(hit_ids[next]);
        }
    }

    // 行クリックでのフォーカス移動
    //（要件: docs/requirements/click-to-focus/）。戻り値は再描画するか。
    //
    // 引数の `line` は描画時のy座標そのもの（zellij は isize で渡してくる）。
    // 行→ペインの対応は render 側のレイアウトから引く。
    // サイドバーは set_selectable(false) のままだが、マウスイベントの配送は
    // selectable にもフォーカスにも縛られない（実測。決定6・16と衝突しない）
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
            focus_pane_with_id(PaneId::Terminal(entry.pane_id), entry.is_floating, false);
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
            &[
                Section("keys"),
                Blank,
                Entry("type", "filter panes"),
                Entry("backspace", "delete char"),
                // クエリ入力と両立しないので、マークとプレビューは Alt付き
                //（決定39・決定42）
                Entry("alt+m", "mark"),
                Entry("alt+v", "preview"),
                Entry("up down", "move cursor"),
                Entry("shift+tab", "move back"),
                Entry("enter", "jump & exit"),
                Entry("esc", "cancel search"),
                Entry("?", "this help"),
            ]
        } else if self.preview.is_some() {
            // プレビュー中だけ既読化キーが増える（決定42）。オフの間は
            // 未定義キー扱いなので、押せないキーをヘルプに残さない
            &[
                Section("keys"),
                Blank,
                Entry("j k", "move"),
                Entry("g G", "top / bottom"),
                Entry("enter", "jump & exit"),
                Entry("/", "search"),
                Entry("p", "triage"),
                Entry("n", "number jump"),
                Entry("m M", "mark / clear all"),
                Entry("v", "preview off"),
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
                Entry("p", "triage"),
                Entry("n", "number jump"),
                Entry("m M", "mark / clear all"),
                Entry("v", "preview"),
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
                        // 使う（決定32）。ここで畳んでおくと、検索・切り詰め・
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
        // 閉じられたペインのマークもここで落とす（決定39。選択のクランプと同じ
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
// 安全弁（決定12）の判定条件で、Shift だけは印字可能文字の一部として素通しする
pub(crate) fn has_hard_modifier(key: &KeyWithModifier) -> bool {
    key.key_modifiers.iter().any(|m| *m != KeyModifier::Shift)
}
