// navモード（決定12）と検索サブモード（要件: docs/requirements/search-explorer/）。
//
// zellij のモード（Ctrl+p の paneモード等）と同じ操作感を、ビルトインモードを
// 潰さずに実現する。config.kdl には入場キー1つだけを書き、モード内のキーは
// InterceptedKeyPress で横取りして解釈する。

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::render::HelpRow;
use crate::search::{match_pane, Hit};
use crate::{Selectable, State};

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
    pub(crate) fn enter_nav_mode(&mut self) {
        eprintln!("fujin: entering nav mode (summoned={})", self.summoned);
        self.nav_mode = true;
        if let Some(pane_id) = self.nav_entry_selection() {
            self.select_pane_id(pane_id);
        }
        self.broadcast_selection();
        intercept_key_presses();
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
        // ヘルプオーバーレイはnavモードの内側の表示。開いたまま退場すると
        // ツリー表示に戻れなくなる（navモード外にキーは届かない）
        self.help_overlay = false;
        // 次の入場で「退場後にフォーカスが動いたか」を判定するために控える
        //（要件: focus-sync）
        self.focus_at_nav_exit = self.focused_pane;
        self.selection_at_nav_exit = self.selectable.get(self.selected).map(|e| e.pane_id);
        // 検索サブモードごと抜ける場合はクエリも破棄する。
        // 次回の入場は常に空クエリから始まる
        self.search = None;
        // トリアージモードも navモードの内側の表示なので、一緒に畳む
        self.triage = None;
        clear_key_presses_intercepts();
        // 召喚インスタンスは用が済んだら自分で退場する（決定16）。
        // 残すと作業ペインに重なり続ける。次の入場でまた呼べばよい
        //（召喚から入場まで実測16ms）
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
        // Shift は素通し（`G` が Shift付きで来る端末があるため）。
        // Ctrl/Alt/Super 付きは未定義なので抜けて安全側に倒す
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        match key.bare_key {
            // 検索サブモードへ（要件: docs/requirements/search-explorer/）
            BareKey::Char('/') => self.enter_search(),
            // トリアージモードへ（要件: docs/requirements/triage-mode/）。
            // `p` は priority の頭文字で、navモード内で未使用だった
            BareKey::Char('p') => self.enter_triage(),
            BareKey::Down | BareKey::Tab | BareKey::Char('j') => self.select_next(),
            BareKey::Up | BareKey::Char('k') => self.select_previous(),
            BareKey::Char('g') => self.selected = 0,
            BareKey::Char('G') => {
                self.selected = self.selectable.len().saturating_sub(1);
            }
            // 1-9 で n 番目へ直行
            BareKey::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index < self.selectable.len() {
                    self.selected = index;
                    self.exit_nav_mode();
                    self.focus_selected();
                }
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
        true
    }

    // 検索サブモード中のキー解釈。navモードの安全弁（決定12）を検索サブモード用に
    // 引き直したもの。印字可能文字はクエリに使うため、1文字ショートカットは全て無効になる
    fn handle_search_key(&mut self, key: KeyWithModifier) -> bool {
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
        true
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
        use HelpRow::{Blank, Entry, Note, Title};
        if self.triage.is_some() {
            self.triage_help_lines()
        } else if self.search.is_some() {
            &[
                Title("[search]", "keys"),
                Blank,
                Entry("type", "filter panes"),
                Entry("backspace", "delete char"),
                Entry("up down tab", "move cursor"),
                Entry("shift+tab", "move back"),
                Entry("enter", "jump & exit"),
                Entry("esc", "cancel search"),
                Entry("?", "this help"),
                Blank,
                Note("press any key to close"),
            ]
        } else {
            &[
                Title("[nav]", "keys"),
                Blank,
                Entry("j k up down tab", "move"),
                Entry("g G", "top / bottom"),
                Entry("1-9", "jump to n"),
                Entry("enter l space", "jump & exit"),
                Entry("/", "search"),
                Entry("p", "triage"),
                Entry("?", "this help"),
                Entry("esc q", "exit"),
                Blank,
                Note("press any key to close"),
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
                        title: pane.title.clone(),
                        is_floating: pane.is_floating,
                    });
                }
            }
        }
        if self.selected >= self.selectable.len() {
            self.selected = self.selectable.len().saturating_sub(1);
        }
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
