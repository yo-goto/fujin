// フォーメーション（要件: docs/requirements/formation/。実装フェーズF1）。
//
// 複数ペインをまとめて名付けたグループ。**メンバーはペイン**で、そのペインで
// 動いているエージェントセッションは付随物として扱う（要件の決定事項）。
//
// データの持ち方は「定義（`formations`）」と「割り当て（`assignments`）」の分離。
// 単一所属モデル（1ペインは常に1フォーメーションのみ）を型で保証するため、
// メンバー一覧を `Formation` 側に持たせない — 両方に持たせると片方だけ更新した
// 食い違った状態を作れてしまう。永続化（F3・F4）で定義ファイルと割り当て
// ファイルを分ける設計（決定44）とも同じ切り方になる。
//
// 編集操作の入力経路は既存のマーク（決定39）を再利用する。専用モードは作らず、
// navモード内の `a`/`x`/`R`/`c` がマーク集合（無ければ注目ペイン1枚）を対象に働く。
//
// 集合はマークと同じく兄弟インスタンスへ配る（決定13）。fujin はタブ数ぶんの
// インスタンスが同時稼働するので、配らないと「どのタブのサイドバーを見ているか」で
// 班の内容が食い違う。
//
// F2 からは**アコーディオンの表示状態**（どちらのセクションにカーソルがあるか・
// splitトグル）も同じ経路で配る。表示状態を配るのは決定13の範囲を広げる判断で、
// 「タブを移っても同じ見え方が続くこと」を優先したもの（2026-08-11）。

use zellij_tile::prelude::*;

use crate::nav::has_hard_modifier;
use crate::render::HelpRow;
use crate::{State, FORMATION_PIPE};

// フォーメーションの定義1件。メンバーは `State::assignments` 側が持つ（上記）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Formation {
    pub(crate) id: u32,
    pub(crate) name: String,
    // 司令（要件: formation-commander）。1フォーメーションにつき0または1で、
    // メンバーのうち1枚を指す。視覚的な区別は色ではなく記号で行う
    pub(crate) commander: Option<u32>,
}

// アコーディオンのどちらのセクションにカーソルがあるか（要件:
// formation-display-accordion）。splitトグル（`State::split`）と合わせて
// **アコーディオンの表示状態**を成し、兄弟インスタンスへ配る（`formation_dump`）。
//
// navモードの入退場ではリセットしない（2026-08-11 決定）。休止中の見え方が
// 入場直後と同じになり、どのタブのサイドバーを見ても揃う
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Section {
    #[default]
    Tabs,
    Formations,
}

// FORMATIONSセクションのカーソル位置。**ツリー側の選択（`State::selected`）と
// 違って行種を持つ** — 見出し行にもカーソルが乗り、そこでは `x`（削除）・
// `a`（直接追加）が名簿行とは別の意味になる（要件: formation-management）。
//
// 位置をインデックスではなくID（フォーメーションID / ペインID）で持つのは
// 決定13・検索カーソルと同じ理由で、一覧が組み直されても同じ行を指せるようにするため
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormationCursor {
    Header(u32),
    Member(u32),
}

// フォーメーション編集のプロンプト（navモード内のミニフロー）。
//
// 検索サブモードの入力欄・終了操作サブモードの確認プロンプトと同じく、モード遷移を
// 増やさずフッターを転用する。権威インスタンス（決定14）にしか発生しないため
// 兄弟インスタンスへは配らない（決定13の範囲外）
#[derive(Debug)]
pub(crate) enum FormationPrompt {
    // 追加先の選択。既存が1件以上あるときだけ出る（0件なら名前入力へ直行する —
    // 選ぶものが無い選択肢を挟んでも1打鍵増えるだけ）
    Pick {
        // 対象ペイン。プロンプトの途中でマークや選択が動いても（兄弟インスタンス
        // からの配布・ペインの増減）押したときに見えていた集合を対象にするため、
        // インデックスではなくペインIDで捕まえておく（終了操作と同じ理由）
        members: Vec<u32>,
    },
    // 名前の入力。新規作成とリネームで共用する
    Name {
        input: String,
        target: NameTarget,
        // 直近の確定が名前の重複で弾かれたか。真のあいだフッターはエラーを出し、
        // 次の入力（文字・Backspace）で入力欄へ戻る
        taken: bool,
    },
    // 見出し行での `x`（フォーメーションの削除）の確認（要件:
    // formation-management）。削除は取り消せないので、終了操作サブモード（決定35）
    // と同じく確認を挟む。対象はプロンプトの途中で一覧が動いてもぶれないよう
    // フォーメーションIDで捕まえておく
    Delete {
        id: u32,
    },
}

// 名前入力の行き先
#[derive(Debug)]
pub(crate) enum NameTarget {
    New { members: Vec<u32> },
    Rename(u32),
}

// 配られたカーソル（`h<フォーメーションID>` / `m<ペインID>`）。読めない値は
// 「カーソルを持っていない」に倒す — 受け手側で先頭へ寄せ直されるだけで済む
fn parse_cursor(raw: &str) -> Option<FormationCursor> {
    let (kind, id) = raw.split_at_checked(1)?;
    let id = id.parse().ok()?;
    match kind {
        "h" => Some(FormationCursor::Header(id)),
        "m" => Some(FormationCursor::Member(id)),
        _ => None,
    }
}

// 追加先の選択で既存フォーメーションに振る番号の上限。`1`-`9` の1打鍵で選ばせる
// ため9件までで、それを超えるぶんはこのプロンプトからは選べない。**10件目から先へは
// 見出し行での `a`（プロンプトを経ない直接追加）で届く**（F2 で入った別経路）
pub(crate) const PICK_LIMIT: usize = 9;

impl State {
    // --- 対象の決め方 ---

    // 編集操作の対象ペイン。マークが1件以上あればマーク集合、0件なら注目ペイン
    // 1枚（要件: 「1ペインだけを対象にする場面が最も多く、毎回マークを強制すると
    // 冗長になる」）。
    //
    // `highlighted_pane()` を使うのはマーク・プレビューと同じ横断的操作だから。
    // トリアージ一覧・検索の絞り込み結果からは要件により Formation 系のキーが
    // 届かないので、実際に引かれるのはツリーの選択行になる
    fn formation_targets(&self) -> Vec<u32> {
        let marked = self.marked_in_tree_order();
        if !marked.is_empty() {
            return marked;
        }
        self.highlighted_pane()
            .map(|pane_id| vec![pane_id])
            .unwrap_or_default()
    }

    // --- 参照 ---

    pub(crate) fn formation(&self, id: u32) -> Option<&Formation> {
        self.formations.iter().find(|f| f.id == id)
    }

    // ペインの所属先（無所属なら None）
    pub(crate) fn formation_of(&self, pane_id: u32) -> Option<u32> {
        self.assignments.get(&pane_id).copied()
    }

    // フォーメーションのメンバーをツリー順（`selectable` の並び）に並べたもの。
    // 名簿の並びはこれで固定する（マークの `marked_in_tree_order` と同じ考え方）。
    //
    // 一覧に無いペインは落ちる — 非可視インスタンスの一覧は古いので名簿が
    // 短く出ることがあるが、割り当てそのものは残る（掃除は下記 `prune_assignments`）
    pub(crate) fn formation_members(&self, id: u32) -> Vec<&crate::Selectable> {
        self.selectable
            .iter()
            .filter(|entry| self.formation_of(entry.pane_id) == Some(id))
            .collect()
    }

    // --- アコーディオンのカーソルと表示状態 ---

    // FORMATIONSセクションに並ぶカーソル位置を上から順に。描画側の行の並び
    // （`render::formation_rows`）と同じ順序で、カーソル移動の土台になる
    pub(crate) fn formation_cursor_rows(&self) -> Vec<FormationCursor> {
        let mut rows = Vec::new();
        for formation in &self.formations {
            rows.push(FormationCursor::Header(formation.id));
            for entry in self.formation_members(formation.id) {
                rows.push(FormationCursor::Member(entry.pane_id));
            }
        }
        rows
    }

    // 実効カーソル。持っている位置が消えていれば先頭へ寄せる（`triage_cursor` と
    // 同じ畳み方で、フォーメーションやメンバーの増減にそのまま追従する）
    pub(crate) fn formation_cursor(&self) -> Option<FormationCursor> {
        let rows = self.formation_cursor_rows();
        self.formation_cursor
            .filter(|cursor| rows.contains(cursor))
            .or_else(|| rows.first().copied())
    }

    // 見出し行にカーソルがあるならそのフォーメーションID。見出し行固有の操作
    //（`x` の削除・`a` の直接追加・`c` の no-op）の入口になる。
    //
    // トリアージ一覧・絞り込み結果からは届かない（要件: それらの一覧は
    // フォーメーションの文脈を持たない）
    pub(crate) fn highlighted_formation(&self) -> Option<u32> {
        if self.section != Section::Formations || self.triage.is_some() || self.search.is_some() {
            return None;
        }
        match self.formation_cursor()? {
            FormationCursor::Header(id) => Some(id),
            FormationCursor::Member(_) => None,
        }
    }

    // カーソルを置き直す。**名簿行へ乗ったときは選択も同じペインへ合わせる** —
    // 名簿行とツリーのペイン行は同じペインを指すので、揃えておけば Enter の
    // ジャンプがツリー側とまったく同じ経路で済む。見出し行では選択を動かさない
    //（指すペインが無い）。
    //
    // カーソルそのものは表示状態として配る（呼び出し側の `broadcast_formations()`）。
    // ツリーの選択（決定13）と同じく**行をIDで運ぶ** — 見出し行は指すペインを
    // 持たないので選択の配布には乗せられず、乗せられる名簿行だけ配ると
    // 兄弟のカーソルが行き先を見失う
    fn set_formation_cursor(&mut self, cursor: FormationCursor) {
        self.formation_cursor = Some(cursor);
        if let FormationCursor::Member(pane_id) = cursor {
            self.select_pane_id(pane_id);
        }
    }

    // FORMATIONSセクションのカーソル移動（`j`/`k`、`g`/`G`）。
    // ツリー側の選択と同じく端で止まる
    pub(crate) fn move_formation_cursor(&mut self, delta: isize) {
        let rows = self.formation_cursor_rows();
        if rows.is_empty() {
            return;
        }
        let at = self
            .formation_cursor()
            .and_then(|cursor| rows.iter().position(|row| *row == cursor))
            .unwrap_or(0);
        let last = rows.len().saturating_sub(1);
        let next = (at as isize + delta).clamp(0, last as isize) as usize;
        self.set_formation_cursor(rows[next]);
        self.broadcast_formations();
    }

    // 先頭・末尾へ（`g`/`G` の FORMATIONSセクション版）
    pub(crate) fn jump_formation_cursor(&mut self, to_end: bool) {
        let rows = self.formation_cursor_rows();
        let target = if to_end { rows.last() } else { rows.first() };
        if let Some(cursor) = target.copied() {
            self.set_formation_cursor(cursor);
            self.broadcast_formations();
        }
    }

    // `Tab` / `Shift+Tab`（要件: formation-display-accordion）。
    //
    // フォーメーションが0件のときは no-op — 切替え先のセクションに中身が無いだけで
    // `Tab` は定義済みのキーなので、**未定義キーの安全弁（決定12）へは落とさない**
    pub(crate) fn toggle_section(&mut self) {
        if self.formations.is_empty() {
            return;
        }
        self.section = match self.section {
            Section::Tabs => Section::Formations,
            Section::Formations => Section::Tabs,
        };
        // 入ってきた側のカーソルを確定させる（名簿行なら選択もそこへ揃う）
        if self.section == Section::Formations {
            if let Some(cursor) = self.formation_cursor() {
                self.set_formation_cursor(cursor);
            }
        }
        self.broadcast_formations();
    }

    // splitトグル（`s`）。両セクションを同時に展開する。0件のときは
    // 切り替える先が無いので `Tab` と同じく no-op
    pub(crate) fn toggle_split(&mut self) {
        if self.formations.is_empty() {
            return;
        }
        self.split = !self.split;
        self.broadcast_formations();
    }

    // フォーメーションが1件も無くなったら表示状態を畳む。セクションが消えた後も
    // `Section::Formations` が残ると、カーソルの行き場が無いまま
    // `highlighted_pane()` が None を返し続ける
    fn normalize_section(&mut self) {
        if self.formations.is_empty() {
            self.section = Section::Tabs;
            self.split = false;
            self.formation_cursor = None;
        }
    }

    // --- 編集操作 ---

    // `a`（追加）。対象が無ければ入らない — 対象の無いプロンプトを出しても
    // どのキーを押しても no-op にしかならない（終了操作の入場と同じ判断）。
    //
    // **見出し行ではプロンプトを経ず、その見出しのフォーメーションへ直接入れる**
    //（要件: formation-management）。行き先が既に決まっているので選ばせる意味が無く、
    // 追加先の選択が9件までしか選べない制約（`PICK_LIMIT`）もこの経路で回避できる
    pub(crate) fn begin_formation_add(&mut self) {
        let members = self.formation_targets();
        if members.is_empty() {
            return;
        }
        if let Some(id) = self.highlighted_formation() {
            self.assign_members(id, &members);
            return;
        }
        self.formation_prompt = Some(if self.formations.is_empty() {
            FormationPrompt::Name {
                input: String::new(),
                target: NameTarget::New { members },
                taken: false,
            }
        } else {
            FormationPrompt::Pick { members }
        });
    }

    // `x`（除外／削除）。**見出し行ではフォーメーションそのものの削除**で、
    // 取り消せないので確認プロンプトを挟む（要件: formation-management）。
    // 名簿行・ペイン行では下の除外になる
    pub(crate) fn remove_from_formation(&mut self) {
        if let Some(id) = self.highlighted_formation() {
            self.formation_prompt = Some(FormationPrompt::Delete { id });
            return;
        }
        self.exclude_from_formation();
    }

    // `x`（除外）。無所属のペインは no-op（要件: 「対象がどのフォーメーションにも
    // 属していない場合は no-op」）。
    //
    // **空になってもフォーメーションは残す**（要件）。名付けた班を一時的に空にして
    // 後で入れ直す運用を想定し、利用者の意図に反して消えないようにする
    fn exclude_from_formation(&mut self) {
        let targets = self.formation_targets();
        let mut changed = false;
        for pane_id in targets {
            if self.assignments.remove(&pane_id).is_some() {
                // 司令のまま抜けさせない。班に居ないペインを指した司令が残ると、
                // 記号だけがどこにも出ない宙ぶらりんの状態になる
                self.demote_commander(pane_id);
                changed = true;
            }
        }
        if changed {
            self.broadcast_formations();
        }
    }

    // `R`（リネーム）。無所属のペインは no-op。既存の名前を初期値に入れて開く —
    // 一部だけ直したい場面が多く、毎回打ち直させると冗長になる。
    // 見出し行ではその見出しのフォーメーションが対象（要件: formation-management）
    pub(crate) fn begin_formation_rename(&mut self) {
        let Some(formation) = self
            .highlighted_formation()
            .or_else(|| {
                self.highlighted_pane()
                    .and_then(|pane_id| self.formation_of(pane_id))
            })
            .and_then(|id| self.formation(id))
        else {
            return;
        };
        let (id, name) = (formation.id, formation.name.clone());
        self.formation_prompt = Some(FormationPrompt::Name {
            input: name,
            target: NameTarget::Rename(id),
            taken: false,
        });
    }

    // `c`（司令のトグル）。無所属のペインは no-op。
    // 新しく設定すると同一フォーメーション内の旧司令は自動的に外れる（`Option` の
    // 上書きがそのまま要件になっている）
    pub(crate) fn toggle_commander(&mut self) {
        let Some(pane_id) = self.highlighted_pane() else {
            return;
        };
        let Some(formation_id) = self.formation_of(pane_id) else {
            return;
        };
        let Some(formation) = self.formations.iter_mut().find(|f| f.id == formation_id) else {
            return;
        };
        formation.commander = if formation.commander == Some(pane_id) {
            None
        } else {
            Some(pane_id)
        };
        self.broadcast_formations();
    }

    // 指定ペインを司令にしている班から司令を外す
    fn demote_commander(&mut self, pane_id: u32) {
        for formation in &mut self.formations {
            if formation.commander == Some(pane_id) {
                formation.commander = None;
            }
        }
    }

    // 集合をフォーメーションへ入れる。単一所属モデルなので、元の所属からは
    // 自動的に外れる（「追加」に「移動」の意味が内包されている）。
    //
    // **マークは消さない。** 終了操作（決定39）がマークを全解除するのは取り消せない
    // 破壊的操作だからで、追加・除外はやり直せるうえ、マークは navモードを退場しても
    // 保持される横断的な集合として設計されている（決定39）
    fn assign_members(&mut self, formation_id: u32, members: &[u32]) {
        for pane_id in members {
            // 班を移るペインを旧班の司令のまま残さない
            self.demote_commander(*pane_id);
            self.assignments.insert(*pane_id, formation_id);
        }
        self.broadcast_formations();
    }

    // 新しいフォーメーションを作ってメンバーを入れる。名前の一意性は
    // 呼び出し側（`confirm_formation_name`）で確かめてある
    fn create_formation(&mut self, name: String, members: &[u32]) {
        let id = self.next_formation_id;
        self.next_formation_id += 1;
        self.formations.push(Formation {
            id,
            name,
            commander: None,
        });
        self.assign_members(id, members);
    }

    // フォーメーションを削除する（見出し行の `x` を確認した後）。定義と一緒に
    // 割り当ても落とし、メンバーは無所属へ戻る（削除はアーカイブ（F7）と違って
    // 復元できない）
    fn delete_formation(&mut self, id: u32) {
        self.formations.retain(|formation| formation.id != id);
        self.assignments
            .retain(|_, formation_id| *formation_id != id);
        self.normalize_section();
        self.broadcast_formations();
    }

    // 名前が既に使われているか（要件: formation-management の一意性）。
    // `except` は自分自身のリネームを弾かないための除外先
    fn name_taken(&self, name: &str, except: Option<u32>) -> bool {
        self.formations
            .iter()
            .any(|f| f.name == name && Some(f.id) != except)
    }

    // --- プロンプトのキー解釈 ---

    // 戻り値は再描画するか。navモード本体のサブモード分岐から呼ばれる
    pub(crate) fn handle_formation_prompt_key(&mut self, key: KeyWithModifier) -> bool {
        // Shift 以外の修飾キーは安全弁（決定12）。サブモードだけでなく navモード
        // ごと抜ける — 抜けられなくなるより退場に倒す
        if has_hard_modifier(&key) {
            self.leave_nav_mode();
            return true;
        }
        match self.formation_prompt {
            Some(FormationPrompt::Pick { .. }) => self.handle_pick_key(key.bare_key),
            Some(FormationPrompt::Name { .. }) => self.handle_name_key(key.bare_key),
            Some(FormationPrompt::Delete { .. }) => self.handle_delete_key(key.bare_key),
            None => false,
        }
    }

    // 削除の確認。**開いたのと同じ `x` で確定**する（終了操作サブモードが
    // 入場キーと確定キーを分けているのと同じで、確認の中は専用のキー空間）
    fn handle_delete_key(&mut self, key: BareKey) -> bool {
        match key {
            BareKey::Esc => self.formation_prompt = None,
            BareKey::Char('x') => {
                if let Some(FormationPrompt::Delete { id }) = self.formation_prompt.take() {
                    self.delete_formation(id);
                }
            }
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        true
    }

    // 追加先の選択。`1`-`9` で既存、`n` で新規作成
    fn handle_pick_key(&mut self, key: BareKey) -> bool {
        match key {
            // Esc は取り消し。サブモードだけ抜けて navモードに留まる（検索・
            // 終了操作と同じパターン）。`exit_nav_mode()` を呼んではいけない —
            // 召喚インスタンスなら取り消しでサイドバーごと閉じてしまう（決定16）
            BareKey::Esc => self.formation_prompt = None,
            // 新規作成へ。対象はそのまま引き継ぐ
            BareKey::Char('n') => {
                if let Some(FormationPrompt::Pick { members }) = self.formation_prompt.take() {
                    self.formation_prompt = Some(FormationPrompt::Name {
                        input: String::new(),
                        target: NameTarget::New { members },
                        taken: false,
                    });
                }
            }
            BareKey::Char(c @ '1'..='9') => self.pick_formation(c),
            // 未定義キーは navモードごと退場（安全弁は最上位まで効かせる）
            _ => self.leave_nav_mode(),
        }
        true
    }

    // 番号で既存フォーメーションを選んで確定する。存在しない番号は
    // プロンプトに留まって打ち直させる（番号ジャンプの 0件と同じ扱い）
    fn pick_formation(&mut self, digit: char) {
        let Some(index) = digit.to_digit(10).map(|d| d as usize).filter(|d| *d >= 1) else {
            return;
        };
        let Some(formation_id) = self
            .formations
            .iter()
            .take(PICK_LIMIT)
            .nth(index - 1)
            .map(|f| f.id)
        else {
            return;
        };
        let Some(FormationPrompt::Pick { members }) = self.formation_prompt.take() else {
            return;
        };
        self.assign_members(formation_id, &members);
    }

    // 名前入力。印字可能文字はそのままクエリではなく名前へ入る
    fn handle_name_key(&mut self, key: BareKey) -> bool {
        match key {
            BareKey::Esc => self.formation_prompt = None,
            BareKey::Enter => self.confirm_formation_name(),
            BareKey::Backspace => {
                if let Some(FormationPrompt::Name { input, taken, .. }) = &mut self.formation_prompt
                {
                    input.pop();
                    // 打ち直しを始めたらエラー表示は畳む
                    *taken = false;
                }
            }
            BareKey::Char(c) => {
                if let Some(FormationPrompt::Name { input, taken, .. }) = &mut self.formation_prompt
                {
                    // 制御文字は入れない。名前は兄弟インスタンスへTSVで運ぶので、
                    // タブ・改行が混ざると行が壊れる（描画側の安全性も同じ理由）
                    if !c.is_control() {
                        input.push(c);
                    }
                    *taken = false;
                }
            }
            _ => self.leave_nav_mode(),
        }
        true
    }

    // 名前を確定する。空白だけの名前と重複はプロンプトに留まって弾く
    fn confirm_formation_name(&mut self) {
        let Some(FormationPrompt::Name { input, target, .. }) = &self.formation_prompt else {
            return;
        };
        let name = input.trim().to_string();
        // 名前の無い班は一覧で見分けられない。エラーは出さず入力を続けさせる
        if name.is_empty() {
            return;
        }
        let except = match target {
            NameTarget::Rename(id) => Some(*id),
            NameTarget::New { .. } => None,
        };
        if self.name_taken(&name, except) {
            if let Some(FormationPrompt::Name { taken, .. }) = &mut self.formation_prompt {
                *taken = true;
            }
            return;
        }
        let Some(FormationPrompt::Name { target, .. }) = self.formation_prompt.take() else {
            return;
        };
        match target {
            NameTarget::New { members } => self.create_formation(name, &members),
            NameTarget::Rename(id) => {
                if let Some(formation) = self.formations.iter_mut().find(|f| f.id == id) {
                    formation.name = name;
                }
                self.broadcast_formations();
            }
        }
    }

    // ヘルプオーバーレイに出すキー一覧（要件: nav-mode-hints）。
    // フッターに収まらない取り消し先もここで補う
    pub(crate) fn formation_prompt_help_lines(&self) -> &'static [HelpRow] {
        use HelpRow::{Blank, Entry, Section};
        match self.formation_prompt {
            Some(FormationPrompt::Pick { .. }) => &[
                Section("keys"),
                Blank,
                Entry("1-9", "pick formation"),
                Entry("n", "new formation"),
                Entry("esc", "cancel"),
                Entry("?", "this help"),
            ],
            Some(FormationPrompt::Delete { .. }) => &[
                Section("keys"),
                Blank,
                Entry("x", "delete formation"),
                Entry("esc", "cancel"),
                Entry("?", "this help"),
            ],
            _ => &[
                Section("keys"),
                Blank,
                Entry("type", "formation name"),
                Entry("backspace", "delete char"),
                Entry("enter", "confirm"),
                Entry("esc", "cancel"),
                Entry("?", "this help"),
            ],
        }
    }

    // --- 掃除 ---

    // 一覧から消えたペインの割り当てを落とす（マークの `prune_marks()` と同じ扱い）。
    // 閉じたペインが名簿に残ると、メンバー数だけが合わない見出しが出る。
    //
    // **空になったフォーメーションは残す**（要件）。司令が閉じられた場合は司令だけ外す
    pub(crate) fn prune_assignments(&mut self) {
        if self.assignments.is_empty() {
            return;
        }
        // 一覧側は不変借用で先に取り出す（`assignments` の可変借用と両立させるため）
        let selectable = &self.selectable;
        let dropped: Vec<u32> = self
            .assignments
            .keys()
            .copied()
            .filter(|pane_id| !selectable.iter().any(|e| e.pane_id == *pane_id))
            .collect();
        for pane_id in dropped {
            self.assignments.remove(&pane_id);
            self.demote_commander(pane_id);
        }
    }

    // --- 兄弟インスタンスへの配布（決定13） ---

    fn broadcast_formations(&self) {
        self.broadcast_to_siblings(FORMATION_PIPE, &self.formation_dump());
    }

    // 新入りインスタンスへの押し付け（決定13）。既存インスタンスが持っている
    // フォーメーションは、配らない限り新入りには一生見えない
    pub(crate) fn push_formations_to(&self, plugin_id: u32) {
        crate::sync::send_to_plugin(plugin_id, FORMATION_PIPE, self.formation_dump());
    }

    // 定義行（`F`）と割り当て行（`A`）を並べたTSV。区切りにタブと改行を使うのは
    // 状態ダンプ（sync.rs）と同じ理由で、名前からは制御文字を弾いてある。
    //
    // 差分ではなく**まるごと**運ぶ（マークと同じ）。差分にすると、取りこぼした
    // 1通ぶんだけ内容が食い違ったまま直らない
    pub(crate) fn formation_dump(&self) -> String {
        let mut out = String::new();
        // アコーディオンの表示状態（`D`）。フォーカス中のセクション・splitトグル・
        // FORMATIONSセクションのカーソルを1行で運ぶ。**表示状態を配るのはこれだけ**
        // — スクロール位置（決定24）は画面高から導出されるので配らない
        out.push_str(&format!(
            "D\t{}\t{}\t{}\n",
            match self.section {
                Section::Tabs => "tabs",
                Section::Formations => "formations",
            },
            u8::from(self.split),
            match self.formation_cursor {
                Some(FormationCursor::Header(id)) => format!("h{}", id),
                Some(FormationCursor::Member(pane_id)) => format!("m{}", pane_id),
                None => String::new(),
            }
        ));
        for formation in &self.formations {
            out.push_str(&format!(
                "F\t{}\t{}\t{}\n",
                formation.id,
                formation.name,
                formation
                    .commander
                    .map(|id| id.to_string())
                    .unwrap_or_default()
            ));
        }
        for (pane_id, formation_id) in &self.assignments {
            out.push_str(&format!("A\t{}\t{}\n", pane_id, formation_id));
        }
        out
    }

    // fujin_formation の受け口。空ペイロードは「フォーメーションが1件も無い」を
    // 意味する（最後の1件を削除した状態も配れるように）
    pub(crate) fn handle_formation_pipe(&mut self, payload: Option<&str>) -> bool {
        payload
            .map(|raw| self.apply_formations(raw))
            .unwrap_or(false)
    }

    // 配られた内容をそのまま採る。戻り値は再描画するか。
    //
    // 受け手側の `selectable` で絞り込まない — 非可視インスタンスの一覧は古く
    // （`PaneUpdate` が届かない）、知らないタブのペインを弾くと配ったそばから
    // 消えてしまう。掃除は一覧を持っている側の `prune_assignments()` に任せる
    // （マークが踏んだのと同じ罠。mark.rs の `apply_marks` 参照）
    pub(crate) fn apply_formations(&mut self, raw: &str) -> bool {
        let mut formations = Vec::new();
        let mut assignments = std::collections::BTreeMap::new();
        // 表示状態は行が無ければ既定（TABS・split なし）。最後の1件を削除した
        // 空ペイロードがそのまま「畳んだ状態」を意味する
        let mut section = Section::Tabs;
        let mut split = false;
        let mut cursor = None;
        for line in raw.lines() {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("D"), Some(focused), Some(split_flag)) => {
                    section = if focused == "formations" {
                        Section::Formations
                    } else {
                        Section::Tabs
                    };
                    split = split_flag == "1";
                    cursor = fields.next().and_then(parse_cursor);
                }
                (Some("F"), Some(id), Some(name)) => {
                    let Ok(id) = id.parse::<u32>() else {
                        continue;
                    };
                    formations.push(Formation {
                        id,
                        name: name.to_string(),
                        commander: fields.next().and_then(|c| c.parse().ok()),
                    });
                }
                (Some("A"), Some(pane_id), Some(formation_id)) => {
                    let (Ok(pane_id), Ok(formation_id)) =
                        (pane_id.parse::<u32>(), formation_id.parse::<u32>())
                    else {
                        continue;
                    };
                    assignments.insert(pane_id, formation_id);
                }
                _ => continue,
            }
        }
        // 定義の無いフォーメーションへの割り当ては落とす。定義ファイルと割り当て
        // ファイルを分ける永続化（決定44）でも同じフォールバックになる ——
        // 定義が消えたペインは暗黙の「無所属」に落ちるだけで済む
        assignments.retain(|_, id| formations.iter().any(|f| f.id == *id));
        if self.formations == formations
            && self.assignments == assignments
            && self.section == section
            && self.split == split
            && self.formation_cursor == cursor
        {
            return false;
        }
        // 自分の採番を配られた最大IDの次まで進めておく。権威インスタンスは
        // 交代しうるので、進めないと交代した先で既存IDを再利用してしまう
        if let Some(max) = formations.iter().map(|f| f.id).max() {
            self.next_formation_id = self.next_formation_id.max(max + 1);
        }
        self.formations = formations;
        self.assignments = assignments;
        self.section = section;
        self.split = split;
        self.formation_cursor = cursor;
        self.normalize_section();
        true
    }
}
