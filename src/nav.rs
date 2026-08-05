// navモード（決定12）と検索サブモード（要件: docs/requirements/search-explorer/）。
//
// zellij のモード（Ctrl+p の paneモード等）と同じ操作感を、ビルトインモードを
// 潰さずに実現する。config.kdl には入場キー1つだけを書き、モード内のキーは
// InterceptedKeyPress で横取りして解釈する。

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::search::{match_pane, Hit};
use crate::{Selectable, State};

// 検索サブモード（navモード内の `/`）のローカルUI状態。
// 権威インスタンスにしか発生しないため、兄弟への同期は不要（決定13の範囲外）
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) query: String,
    // ペインID -> ヒット情報。ツリー順は selectable 側が持つので順序は持たない
    pub(crate) hits: BTreeMap<u32, Hit>,
    // 結果内の選択（ペインID）。インデックスで持つと rebuild_selectable() を
    // またいだときに別の行を指す（決定13が禁じた罠のローカル版）
    pub(crate) cursor: Option<u32>,
    // 検索に入る前の選択（Esc で戻すため）
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
        // 次の入場で「退場後にフォーカスが動いたか」を判定するために控える
        //（要件: focus-sync）
        self.focus_at_nav_exit = self.focused_pane;
        self.selection_at_nav_exit = self.selectable.get(self.selected).map(|e| e.pane_id);
        // 検索サブモードごと抜ける場合はクエリも破棄する。
        // 次回の入場は常に空クエリから始まる
        self.search = None;
        clear_key_presses_intercepts();
        // 臨時召喚されたインスタンスは用が済んだら自分で退場する（決定16）。
        // 残すと作業ペインに重なり続ける。次の入場でまた呼べばよい
        //（召喚から入場まで実測16ms）
        if self.summoned {
            if let Some(own_id) = self.own_plugin_id {
                close_plugin_pane(own_id);
            }
        }
    }

    // ジャンプを伴わない離脱（Esc / q / 未定義キー、および navモード中に実フォーカスが
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

    // ペインIDで選択行を移す。戻り値は選択が動いたか。
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

    // モード中のキー解釈。戻り値は再描画するか
    pub(crate) fn handle_nav_key(&mut self, key: KeyWithModifier) -> bool {
        // 検索サブモード中は専用ハンドラへ。下の修飾キー判定より手前に
        // 置くこと — 検索側は Shift+Tab（修飾付き）を通す必要がある
        if self.search.is_some() {
            return self.handle_search_key(key);
        }
        // Shift は素通し（`G` が Shift付きで来る端末があるため）。
        // Ctrl/Alt/Super 付きは未定義なので抜けて安全側に倒す
        if key.key_modifiers.iter().any(|m| *m != KeyModifier::Shift) {
            self.leave_nav_mode();
            return true;
        }
        match key.bare_key {
            // 検索サブモードへ（要件: docs/requirements/search-explorer/）
            BareKey::Char('/') => self.enter_search(),
            BareKey::Down | BareKey::Tab | BareKey::Char('j') => {
                if self.selected + 1 < self.selectable.len() {
                    self.selected += 1;
                }
            }
            BareKey::Up | BareKey::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
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
            // Esc / q は明示的な離脱。それ以外の未定義キーでも抜ける:
            // 万一プラグインが応答不能になってもキー入力が取り残されないため
            _ => self.leave_nav_mode(),
        }
        // 横取り中の移動は自分にしか起きないので、都度配る
        self.broadcast_selection();
        true
    }

    // 検索中のキー解釈。navモードの安全弁（決定12）を検索用に引き直したもの。
    // 印字可能文字はクエリに使うため、1文字ショートカットは全て無効になる
    fn handle_search_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift だけは素通し（Shift付き印字可能文字と Shift+Tab のため）。
        // それ以外の修飾キーは安全弁 — 検索だけでなく navモードごと離脱する
        if key.key_modifiers.iter().any(|m| *m != KeyModifier::Shift) {
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
            // 未定義キーは navモードごと離脱（安全弁は最上位まで効かせる）
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
            query: String::new(),
            hits: BTreeMap::new(),
            cursor: saved,
            saved,
        });
        self.refilter();
    }

    // Esc の1段目。クエリを破棄し、選択を検索前に戻して navモードに留まる
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
        // self.search を可変借用したまま selectable / tabs を読めないので、
        // ローカルに組み立ててから代入する
        let query = search.query.clone();
        let mut hits: BTreeMap<u32, Hit> = BTreeMap::new();
        for entry in &self.selectable {
            let tab_name = self
                .tabs
                .iter()
                .find(|t| t.position == entry.tab_position)
                .map(|t| t.name.as_str())
                .unwrap_or("");
            let cwd = self.pane_cwds.get(&entry.pane_id).map(String::as_str);
            if let Some(hit) = match_pane(&query, &entry.title, tab_name, cwd) {
                hits.insert(entry.pane_id, hit);
            }
        }
        // カーソルの追従はペインIDで行う。結果に残っていれば維持し、
        // 消えていればツリー順の先頭ヒットへ寄せる。0件なら None
        let cursor = self
            .search
            .as_ref()
            .and_then(|s| s.cursor)
            .filter(|id| hits.contains_key(id))
            .or_else(|| {
                self.selectable
                    .iter()
                    .map(|e| e.pane_id)
                    .find(|id| hits.contains_key(id))
            });
        if let Some(search) = &mut self.search {
            search.hits = hits;
            search.cursor = cursor;
        }
    }

    // 検索結果内のカーソル移動。ツリー順で前後へ動かし、端で止まる
    fn move_search_cursor(&mut self, forward: bool) {
        let Some(search) = &self.search else {
            return;
        };
        let results: Vec<u32> = self
            .selectable
            .iter()
            .map(|e| e.pane_id)
            .filter(|id| search.hits.contains_key(id))
            .collect();
        if results.is_empty() {
            return;
        }
        let current = search
            .cursor
            .and_then(|c| results.iter().position(|&id| id == c));
        let next = match current {
            Some(i) if forward => (i + 1).min(results.len() - 1),
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        if let Some(search) = &mut self.search {
            search.cursor = Some(results[next]);
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
        // navモード中にマウスが届いた場合も Enter と同じ扱いにする（v1の要件は
        // 通常時のクリックのみだが、横取りを残したままフォーカスだけ動かすと
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
            // かといって常に true にすると、今度は**フローティング表示中に
            // タイルペインへ戻れなくなる**。どちらも実測で確認済み
            focus_pane_with_id(PaneId::Terminal(entry.pane_id), entry.is_floating, false);
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
    }
}
