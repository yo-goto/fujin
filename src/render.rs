// サイドバーの描画。
//
// レイアウト: ヘッダ1行（セッション名 / navモード名 / 検索クエリ）に続けて、
// タブ見出し > 配下のペイン行 をタブ順で縦に並べる。

use zellij_tile::prelude::*;

use crate::search::{Field, Hit};
use crate::{Selectable, State};

impl State {
    pub(crate) fn draw(&self, rows: usize, cols: usize) {
        if !self.permissions_granted {
            print_text_with_coordinates(
                Text::new("permissions required (press y)"),
                0,
                0,
                None,
                None,
            );
            return;
        }
        let mut y = 0;
        // ヘッダ: 検索中はクエリ入力行、navモード中はモード名、通常はセッション名
        if let Some(search) = &self.search {
            let header = truncate(&format!("/{}▏", search.query), cols);
            print_text_with_coordinates(
                Text::new(&header).color_range(3, ..header.chars().count()),
                0,
                y,
                None,
                None,
            );
            y += 1;
            // 0件は空リストではなく明示する。絞り込みが効いているのか
            // 描画が壊れているのか区別できないため
            if search.hits.is_empty() {
                if y < rows {
                    print_text_with_coordinates(Text::new("  一致なし"), 0, y, None, None);
                }
                return;
            }
        } else if self.nav_mode {
            let header = truncate("-- NAV --  j/k ↵ esc", cols);
            print_text_with_coordinates(
                Text::new(&header).color_range(3, ..header.chars().count()),
                0,
                y,
                None,
                None,
            );
            y += 1;
        } else if let Some(name) = &self.session_name {
            let header = truncate(name, cols);
            print_text_with_coordinates(
                Text::new(&header).color_range(2, ..header.chars().count()),
                0,
                y,
                None,
                None,
            );
            y += 1;
        }

        let mut flat_index = 0;
        let mut sorted_tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        sorted_tabs.sort_by_key(|t| t.position);
        for tab in sorted_tabs {
            if y >= rows {
                break;
            }
            // 絞り込み中、配下に一致ペインを持たないタブは見出しごと消す。
            // flat_index は非検索時の選択にしか使わないので、間引いてもずれない
            if let Some(search) = &self.search {
                let has_hit = self.selectable.iter().any(|e| {
                    e.tab_position == tab.position && search.hits.contains_key(&e.pane_id)
                });
                if !has_hit {
                    continue;
                }
            }
            print_text_with_coordinates(self.tab_heading(tab, cols), 0, y, None, None);
            y += 1;

            for entry in &self.selectable {
                if entry.tab_position != tab.position {
                    continue;
                }
                let this_index = flat_index;
                flat_index += 1;
                // 絞り込みで外れたペインは描かない
                let hit = self
                    .search
                    .as_ref()
                    .and_then(|s| s.hits.get(&entry.pane_id));
                if self.search.is_some() && hit.is_none() {
                    continue;
                }
                if y >= rows {
                    break;
                }
                // 検索中の選択は結果内カーソル（ペインID）で決まる
                let is_selected = match &self.search {
                    Some(search) => search.cursor == Some(entry.pane_id),
                    None => this_index == self.selected,
                };
                let row = self.pane_row(entry, is_selected, hit, cols);
                print_text_with_coordinates(row, 0, y, None, None);
                y += 1;
            }
        }
    }

    // タブ見出し1行ぶんの Text を組み立てる
    fn tab_heading(&self, tab: &TabInfo, cols: usize) -> Text {
        let marker = if tab.active { "▾" } else { "▸" };
        let prefix = format!("{} {} ", marker, tab.position + 1);
        let full_heading = format!("{}{}", prefix, tab.name);
        let title = truncate(&full_heading, cols);
        let mut text = Text::new(&title);
        if tab.active {
            text = text.color_range(0, ..title.chars().count());
        }
        // タブ名にヒットしたら見出し側をハイライトする（ヒット箇所の提示）。
        // 配下のどのペインの Hit も同じタブ名を指すので、最初の1つで足りる
        if let Some(search) = &self.search {
            let tab_hit = self
                .selectable
                .iter()
                .filter(|e| e.tab_position == tab.position)
                .find_map(|e| {
                    search
                        .hits
                        .get(&e.pane_id)
                        .filter(|h| h.field == Field::Tab)
                });
            if let Some(hit) = tab_hit {
                let indices = shift_highlight_indices(
                    &hit.indices,
                    prefix.chars().count(),
                    &title,
                    full_heading.chars().count(),
                );
                if !indices.is_empty() {
                    text = text.color_indices(1, indices);
                }
            }
        }
        text
    }

    // ペイン1行ぶんの Text を組み立てる（アイコン・カウンタ・cwd・ハイライト込み）
    fn pane_row(
        &self,
        entry: &Selectable,
        is_selected: bool,
        hit: Option<&Hit>,
        cols: usize,
    ) -> Text {
        let agent = self.agents.get(&entry.pane_id);
        let icon = agent.map(|a| a.state.icon()).unwrap_or(" ");
        // 選択行は左端にバーを立てる。テーマの選択色が沈む配色でも
        // どこが選択中か一目で分かるようにするため（幅は2文字で固定し、
        // アイコンの color_range 2..3 をずらさない）
        let prefix = if is_selected { "▌ " } else { "  " };
        let mut label = format!("{}{} {}", prefix, icon, entry.title);
        if let Some(a) = agent {
            if a.subagents > 0 {
                label.push_str(&format!(" +{}", a.subagents));
            }
            if a.open_tasks > 0 {
                label.push_str(&format!(" [{}]", a.open_tasks));
            }
        }
        // cwd にヒットしたペインは show_cwd が false でも cwd を出す。
        // 画面に無い文字列でヒットしたように見せないため
        let show_cwd_here = self.show_cwd || matches!(hit, Some(h) if h.field == Field::Cwd);
        let mut cwd_offset = None;
        if show_cwd_here {
            if let Some(cwd) = self.pane_cwds.get(&entry.pane_id) {
                cwd_offset = Some(label.chars().count() + 2);
                label.push_str(&format!("  {}", cwd));
            }
        }
        let full_len = label.chars().count();
        let mut label = truncate(&label, cols);
        // ハイライトする場所が、そのままヒットしたフィールドの提示になる
        let highlight = hit.and_then(|hit| {
            let offset = match hit.field {
                Field::Title => Some(4), // "▌ {icon} " の4文字ぶん
                Field::Cwd => cwd_offset,
                Field::Tab => None, // タブ見出し側で描いている
            };
            offset.map(|o| shift_highlight_indices(&hit.indices, o, &label, full_len))
        });
        if is_selected {
            // 選択背景がサイドバー幅いっぱいに伸びるよう空白で埋める。
            // 埋めないと文字列の長さぶんしか色が乗らず、帯に見えない
            let pad = cols.saturating_sub(label.chars().count());
            label.push_str(&" ".repeat(pad));
        }
        let mut text = Text::new(&label);
        if let Some(a) = agent {
            // アイコン部分（先頭2..3文字目）に状態色
            text = text.color_range(a.state.color(), 2..3);
        }
        if let Some(indices) = highlight.filter(|i| !i.is_empty()) {
            // レベル1で固定（決定11のv1スコープ: 設定項目は増やさない）
            text = text.color_indices(1, indices);
        }
        if is_selected {
            // opaque を付けないと背景が透けて選択色が沈む
            text = text.selected().opaque().color_range(2, 0..1);
        }
        text
    }
}

// マッチ位置（フィールド内の char index）を行ラベル内の位置へずらし、
// truncate() で切られて画面に無い位置を捨てる
pub(crate) fn shift_highlight_indices(
    indices: &[usize],
    offset: usize,
    truncated: &str,
    original_len: usize,
) -> Vec<usize> {
    let visible = truncated.chars().count();
    // 切り詰められた行の末尾は … なので、そこには色を乗せない
    let limit = if visible < original_len {
        visible.saturating_sub(1)
    } else {
        visible
    };
    indices
        .iter()
        .map(|i| i + offset)
        .filter(|i| *i < limit)
        .collect()
}

// 文字数ベースの単純切り詰め（v1: CJK幅は考慮しない）
pub(crate) fn truncate(s: &str, max: usize) -> String {
    // 幅0のときに省略記号だけがはみ出さないようにする
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
