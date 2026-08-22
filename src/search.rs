// 検索サブモード（navモード内の `/`、決定202608031908。要件: .docs/requirements/req-search-explorer.md）。
//
// 絞り込みのキー操作と、その結果を持つローカルUI状態を担う。
// 編集状態/操作状態の2状態に分かれる（決定202608131200）。
//
// ファジーマッチそのものは `matcher` に分けてある。あちらは zellij のホスト関数にも
// State にも依存しない純粋ロジックで、その境界を保つためにファイルを分けている。

mod matcher;

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

pub(crate) use matcher::{match_pane, Field, Hit};

use crate::nav::has_hard_modifier;
use crate::State;

// 検索サブモードの2状態（決定202608131200。要件:
// features/search-explorer/search-mode-key-handling.feature）。
// vim の挿入/ノーマルに相当する分割で、`?` のクエリ入力と `j`/`k` 移動を両立させる
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPhase {
    // 編集状態: 印字可能文字（`?` を含む）はすべてクエリへ積む。`/` で入った直後の既定
    #[default]
    Editing,
    // 操作状態: `j`/`k` を含む移動キーでカーソルを動かし、`?` でヘルプを開く。
    // コマンドキー以外は無反応（押し間違いで状態が黙って変わる事故を避ける）
    Navigating,
}

// 検索サブモード（navモード内の `/`）のローカルUI状態。
// 権威インスタンスにしか発生しないため、兄弟への同期は不要（決定202608012141の範囲外）
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    // 編集状態/操作状態（決定202608131200）。キー処理・フッターの出し分けの軸
    pub(crate) phase: SearchPhase,
    pub(crate) query: String,
    // ペインID -> ヒット情報。ツリー順は selectable 側が持つので順序は持たない
    pub(crate) hits: BTreeMap<u32, Hit>,
    // 絞り込み結果内のカーソル（ペインID）。インデックスで持つと
    // rebuild_selectable() をまたいだときに別の行を指す（決定202608012141が禁じた罠のローカル版）
    pub(crate) cursor: Option<u32>,
    // 検索サブモードに入る前の選択（Esc で戻すため）
    pub(crate) saved: Option<u32>,
}

impl State {
    // 検索サブモード中のキー解釈。navモードの安全弁（決定202607310311）を検索サブモード用に
    // 引き直したもの。印字可能文字はクエリに使うため、1文字ショートカットは全て無効になる
    pub(crate) fn handle_search_key(&mut self, key: KeyWithModifier) -> bool {
        // マーク（決定202608080250）とプレビュー（決定202608082045）は絞り込み結果の上でも使えるよう、
        // 安全弁（下の has_hard_modifier）の例外として Alt付きで通す。
        // マーク全解除（Esc で戻ってから押せばよい）と既読化（取り消せない操作）は
        // この例外を広げない
        if key.bare_key == BareKey::Char('m') && key.key_modifiers.contains(&KeyModifier::Alt) {
            self.toggle_mark();
            return true;
        }
        if key.bare_key == BareKey::Char('p') && key.key_modifiers.contains(&KeyModifier::Alt) {
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
        // 状態で使えるキーが変わる（決定202608131200）。`?` はここへ来る前に
        // handle_nav_key が拾う（操作状態ならヘルプ、編集状態なら下の Char へ）
        match self.search_phase() {
            SearchPhase::Editing => self.handle_search_editing_key(&key, shifted),
            SearchPhase::Navigating => {
                if !self.handle_search_navigating_key(&key, shifted) {
                    // コマンドキー以外は無反応（決定202608131200）。自動で編集状態へ戻して
                    // クエリへ積む案は、押し間違いでクエリが汚れるので採らない。
                    // 描き直しも起こさない — 画面はどこも変わっていない
                    return false;
                }
            }
        }
        // 検索中の移動・入力では broadcast_selection() を呼ばない。
        // Esc で「検索前の位置に戻す」以上、途中経過を配ると兄弟だけが
        // 取り消せない位置に取り残される（決定202608012141）。配るのは確定時
        //（confirm_search）だけ
        //
        // プレビューは配布ではなく自分の表示なので、絞り込みのカーソルにも
        // そのまま追従させる（決定202608082045）
        self.refresh_preview();
        true
    }

    // 編集状態のキー解釈（決定202608131200）。決定202608031908 の挙動から `?` の特別扱いだけを外した形で、
    // 印字可能文字は `?` を含めすべてクエリへ積む
    fn handle_search_editing_key(&mut self, key: &KeyWithModifier, shifted: bool) {
        match key.bare_key {
            // Esc はクエリを持ったまま操作状態へ移るだけ（決定202608131200でここが変わった。
            // 以前はこの段でクエリを破棄していた）
            BareKey::Esc => self.set_search_phase(SearchPhase::Navigating),
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
    }

    // 操作状態のキー解釈（決定202608131200）。戻り値は**コマンドキーとして解釈したか**で、
    // false ならそのキーは無反応（安全弁にも倒さない。vimのnormalモードに近い
    // 予測可能性を優先し、押し間違いで状態が黙って変わる事故を避ける）
    fn handle_search_navigating_key(&mut self, key: &KeyWithModifier, shifted: bool) -> bool {
        match key.bare_key {
            // ここで初めてクエリを破棄して navモードのツリー表示へ戻る
            //（決定202608031908の1段目Escに相当。Esc は決定202608131200で三段階になった）。
            // exit_nav_mode() を呼んではいけない — 召喚インスタンスなら
            // 検索の取り消しでサイドバーごと閉じてしまう（決定202608011644）
            BareKey::Esc => self.exit_search(),
            BareKey::Enter => self.confirm_search(),
            BareKey::Up | BareKey::Char('k') => self.move_search_cursor(false),
            BareKey::Down | BareKey::Char('j') => self.move_search_cursor(true),
            BareKey::Tab => self.move_search_cursor(!shifted),
            // 編集の再開は専用キーに限る（vim由来。navモードで未使用のキー）
            BareKey::Char('i') => self.set_search_phase(SearchPhase::Editing),
            _ => return false,
        }
        true
    }

    // いまの検索サブモードの状態。検索中でなければ既定（編集状態）を返す —
    // 呼び出し元は検索中しか通らないので、この値は使われない
    fn search_phase(&self) -> SearchPhase {
        self.search.as_ref().map(|s| s.phase).unwrap_or_default()
    }

    // 検索サブモードの編集状態にいるか。`?` の最優先チェックの例外条件（決定202608131200）
    pub(crate) fn search_is_editing(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|s| s.phase == SearchPhase::Editing)
    }

    fn set_search_phase(&mut self, phase: SearchPhase) {
        if let Some(search) = &mut self.search {
            search.phase = phase;
        }
    }

    pub(crate) fn enter_search(&mut self) {
        let saved = self.selectable.get(self.selected).map(|e| e.pane_id);
        self.search = Some(SearchState {
            cursor: saved,
            saved,
            ..SearchState::default()
        });
        self.refilter();
    }

    // 操作状態の Esc（決定202608131200で三段階になったうちの2段目）。クエリを破棄し、
    // 検索サブモードに入る前の選択へ戻して navモードに留まる
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
        self.broadcast_selection(); // 確定時だけ配る（決定202608012141）
        self.focus_selected();
    }

    // 一括で届くテキスト入力（貼り付けと、**IMEの変換確定**）の受け口。
    //
    // 複数文字が一度に来る入力は `InterceptedKeyPress` ではなく `PastedText` に
    // 分かれる（zellij クライアントの入力ハンドラが、まとまった文字列を
    // `InputEvent::Paste` として解釈するため）。購読していないと、変換で確定した
    // 文字列が丸ごと消えたように見える（.docs/issues/issue-ime-input-support.md）。
    //
    // navモードの安全弁（決定202607310311）はここには効かせない — 未定義の**キー**で抜ける
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
        // 操作状態はテキストを受け付けない（決定202608131200。キーと同じく無反応）。
        // 画面ではクエリを dim にして「いま打てない」と示している
        if search.phase != SearchPhase::Editing {
            return false;
        }
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
    pub(crate) fn refilter(&mut self) {
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
}
