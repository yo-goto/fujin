// 行の並びと表示範囲（要件: features/sidebar-tree/sidebar-scroll.feature）。
//
// ツリーに積む `Row` の一覧を組み立て、画面高に合わせて切り出す。描画（`draw`）と
// クリック位置の逆引き（`pane_at_row`）が同じ並びを共有するための土台。

use super::*;

impl State {
    // このフレームにマーク列を出すか（決定202608080250）。カウンタ列と同じく**そのフレームに
    // 出る行の実測**で決める — 誰もマークしていないフレームでは列そのものが消え、
    // ペイン名が幅をすべて使う。マーク済みの行が1つでもあれば、同じフレームの
    // マークされていない行も空白で列ぶんを空けてアイコンの位置を揃える
    pub(crate) fn mark_column(&self, rows: &[Row<'_>]) -> bool {
        rows.iter().any(|row| match row {
            Row::Pane { entry, .. } | Row::Triage { entry, .. } => self.is_marked(entry.pane_id),
            _ => false,
        })
    }

    // このフレームのカウンタ列の幅。visible_rows() の結果から測るので、
    // 絞り込みで消えたペインは勘定に入らない
    pub(crate) fn counter_column(&self, rows: &[Row<'_>]) -> CounterColumn {
        let mut column = CounterColumn::default();
        for row in rows {
            let Row::Pane { entry, .. } = row else {
                continue;
            };
            let (subagents, open_tasks) = counter_labels(self.agents.get(&entry.pane_id));
            column.subagents = column.subagents.max(UnicodeWidthStr::width(&*subagents));
            column.open_tasks = column.open_tasks.max(UnicodeWidthStr::width(&*open_tasks));
        }
        column
    }

    // 画面に並ぶ行を上から順に組み立てる。`rows`（画面高）での打ち切りは
    // 呼び出し側の責務 — 行の並び自体は高さに依らないため。
    //
    // 枠（境界線・ヘッダー・フッター）は**常時 FRAME_TOP + FRAME_BOTTOM 行を
    // 確保する**（要件: sidebar-header / sidebar-footer）。中身がモードで
    // 変わっても行数は変えない — 入退場で枠の高さが動くと、ツリー全体が
    // そのぶん上下にずれる
    pub(crate) fn visible_rows(&self) -> Vec<Row<'_>> {
        if !self.permissions_granted {
            return Vec::new();
        }
        let content = self.content_rows();
        let mut rows = Vec::with_capacity(content.len() + FRAME_TOP + FRAME_BOTTOM);
        rows.push(Row::Divider);
        rows.push(Row::Header);
        rows.push(Row::Divider);
        rows.extend(content);
        rows.push(Row::Divider);
        rows.push(Row::Footer);
        // status-bar との間に空ける1行（FRAME_BOTTOM のコメント参照）
        rows.push(Row::Blank);
        rows
    }

    // 枠の内側（content）に並ぶ行。ヘルプオーバーレイ表示中はツリーの代わりに
    // ヘルプ内容が入る — 覆うのは content だけで、枠は出したままにする（決定202608070119）
    fn content_rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        if self.help_overlay {
            // キー一覧（モードごとに違う）＋ 状態アイコン凡例（共通）。
            // 収まらないぶんはツリーと同じあふれマーカーで示す — オーバーレイは
            // 「任意のキーで閉じる」ので、スクロール用のキーを持てない
            return self
                .help_lines()
                .iter()
                .chain(STATUS_LEGEND.iter())
                .map(Row::Help)
                .collect();
        }
        // トリアージモード中はツリー表示を隠し、一覧だけを出す（要件: triage-mode）
        if self.triage.is_some() {
            let entries = self.triage_entries();
            if entries.is_empty() {
                rows.push(Row::Notice("nothing to triage"));
                return rows;
            }
            for entry in entries {
                rows.push(Row::Triage {
                    entry,
                    tab_name: self.tab_name(entry.tab_position),
                });
            }
            return rows;
        }
        if let Some(search) = &self.search {
            // 0件は空リストではなく明示する。絞り込みが効いているのか
            // 描画が壊れているのか区別できないため
            if search.hits.is_empty() {
                rows.push(Row::Notice("no matches"));
                return rows;
            }
        }

        let mut flat_index = 0;
        let mut sorted_tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        sorted_tabs.sort_by_key(|t| t.position);
        for tab in sorted_tabs {
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
            rows.push(Row::Tab(tab));

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
                rows.push(Row::Pane {
                    entry,
                    flat_index: this_index,
                    hit,
                });

                // cwd はペイン行に混ぜず、続く1行として出す（決定202608060053）。
                // show_cwd が false でも、cwd に一致した行だけは出す —
                // 画面に無い文字列でヒットしたように見せないため。
                // ただしペイン名フォールバック中の行では出さない。ペイン名の位置に
                // 既に同じパスが出ており、2行並べても情報が増えない（決定202608070102）
                let cwd_hit = hit.filter(|h| h.field == Field::Cwd);
                // エージェントが終了したペインでは出さない
                //（.docs/issues/issue-sidebar-cwd-persists-after-exit.md）。cwd はフック由来
                // なので `pane_cwds` はエージェントが去った後も残るが、show_cwd が
                // 見せたいのは動いているエージェントの居場所。終了後も出し続けると、
                // シェルがタイトルを cwd に戻した瞬間から同じパスが2行並ぶ。
                // 検索ヒットの側はこの条件を通さない — 一覧に出ている以上、
                // 何に一致したかは示す必要がある
                let show_for_agent = self.show_cwd && self.agents.contains_key(&entry.pane_id);
                if (show_for_agent || cwd_hit.is_some()) && self.title_fallback(entry).is_none() {
                    if let Some(cwd) = self.display_cwd(entry.pane_id) {
                        rows.push(Row::Cwd {
                            entry,
                            flat_index: this_index,
                            cwd,
                            hit: cwd_hit,
                        });
                    }
                }
            }
        }
        rows
    }

    // 画面に実際に載る行。visible_rows() の並びから表示範囲ぶんを切り出し、
    // 隠れた行があれば上下端にあふれマーカー行を足す。
    //
    // **返す行数は常に画面高ぴったり**（画面が枠より低いときを除く）。ツリーが
    // 短いぶんは空行で埋め、下の枠を最下部へ押し下げる — 埋めないとフッターが
    // ツリーの直後に浮き、画面高で位置が動いてしまう。
    //
    // `rows` が 0 のときは切り出さない。行クリックの逆引きが最初の描画より前に
    // 来た場合（State::viewport_rows の初期値）で、スクロールは起きていない
    pub(crate) fn screen_rows(&self, rows: usize) -> Vec<Row<'_>> {
        let mut all = self.visible_rows();
        if rows == 0 || all.is_empty() {
            return all;
        }
        let frame = FRAME_TOP + FRAME_BOTTOM;
        // 画面が枠ぶんの高さも無いときは、入るところまでを出して終わる。
        // 一覧に割ける高さが無いので、スクロールもあふれマーカーも出番がない
        if rows <= frame {
            all.truncate(rows);
            return all;
        }
        let area = rows - frame;
        let list_len = all.len() - frame;
        // 描画とクリックの逆引きで同じ位置を使う。State::scroll は描画時に
        // 寄せた値だが、そのあと一覧が縮んでいることもあるので clamp は掛け直す
        let scroll = reconcile_scroll(list_len, area, self.scroll, None);
        let shown = rows_shown(list_len, area, scroll);

        // 上の枠 / 一覧 / 下の枠 の3つに割る（並びは visible_rows() が決めている）
        let bottom = all.split_off(FRAME_TOP + list_len);
        let list = all.split_off(FRAME_TOP);
        let mut screen = all;
        if scroll > 0 {
            screen.push(Row::Overflow {
                hidden: scroll,
                above: true,
            });
        }
        screen.extend(list.into_iter().skip(scroll).take(shown));
        let below = list_len.saturating_sub(scroll + shown);
        if below > 0 {
            screen.push(Row::Overflow {
                hidden: below,
                above: false,
            });
        }
        // 余った高さを空行で埋めてから下の枠を置く
        screen.resize_with(FRAME_TOP + area, || Row::Blank);
        screen.extend(bottom);
        screen
    }

    // 光っている行（ツリー表示では選択、検索・トリアージではカーソル）が
    // visible_rows() のどこにあるか（先頭行, 末尾行）。
    // ペイン行と cwd行のように複数行が1つの帯になるので範囲で返す
    fn highlighted_span(&self, all: &[Row<'_>]) -> Option<(usize, usize)> {
        let triage_cursor = self.triage_cursor();
        let mut span: Option<(usize, usize)> = None;
        for (index, row) in all.iter().enumerate() {
            let selected = match row {
                Row::Pane {
                    entry, flat_index, ..
                }
                | Row::Cwd {
                    entry, flat_index, ..
                } => self.row_is_highlighted(entry, *flat_index),
                Row::Triage { entry, .. } => triage_cursor == Some(entry.pane_id),
                _ => false,
            };
            if selected {
                span = Some(match span {
                    Some((first, _)) => (first, index),
                    None => (index, index),
                });
            }
        }
        span
    }

    // 表示範囲を選択行へ寄せ直す。描画のたびに呼ぶ（画面高は描画時にしか
    // 分からず、行の増減も選択の移動もここで一度に吸収できるため）。
    //
    // スクロール位置は選択（決定202608012141で兄弟インスタンスへ配る）と画面高から
    // 導出されるローカルな表示状態なので、それ自体は配らない
    pub(crate) fn reconcile_viewport(&mut self, rows: usize) {
        self.viewport_rows = rows;
        // ヘルプオーバーレイはツリーとは別の面（決定202608070119）。ツリーのスクロール位置を
        // 持ち込むと、開いた瞬間に途中の行から表示される
        if self.help_overlay {
            self.scroll = 0;
            return;
        }
        let frame = FRAME_TOP + FRAME_BOTTOM;
        let (list_len, area, anchor) = {
            let all = self.visible_rows();
            let anchor = self.highlighted_span(&all).map(|(first, last)| {
                (
                    first.saturating_sub(FRAME_TOP),
                    last.saturating_sub(FRAME_TOP),
                )
            });
            (
                all.len().saturating_sub(frame),
                rows.saturating_sub(frame),
                anchor,
            )
        };
        self.scroll = reconcile_scroll(list_len, area, self.scroll, anchor);
    }

    // 画面のこの行に載っているペイン（要件: .docs/requirements/req-click-to-focus.md）。
    // ヘッダ・タブ見出し行・あふれマーカー行・一覧の外は None。
    // cwd行はペイン行と同じペインを指すので、そこをクリックしても同じように当たる
    pub(crate) fn pane_at_row(&self, row: usize) -> Option<u32> {
        match self.screen_rows(self.viewport_rows).get(row)? {
            Row::Pane { entry, .. } | Row::Cwd { entry, .. } | Row::Triage { entry, .. } => {
                Some(entry.pane_id)
            }
            _ => None,
        }
    }

    // タブ位置に対応するタブ名。一覧が古くて引けないときは空文字
    pub(crate) fn tab_name(&self, position: usize) -> &str {
        self.tabs
            .iter()
            .find(|t| t.position == position)
            .map(|t| t.name.as_str())
            .unwrap_or("")
    }

    // その行がハイライトされるか。ペイン行と cwd行で同じ判定を使い、
    // 2行が1つの帯に見えるようにする
    pub(super) fn row_is_highlighted(&self, entry: &Selectable, flat_index: usize) -> bool {
        match &self.search {
            // 検索サブモード中にハイライトする行はカーソル（ペインID）で決まる
            Some(search) => search.cursor == Some(entry.pane_id),
            None => flat_index == self.selected,
        }
    }
}

// 表示範囲の外に隠れている行があることを示す1行。文言は英語で統一する。
// 記号はタブ見出し行と同じ三角の系列で、上下どちら側が隠れているかを向きで示す。
// タブ見出しと同列（x=0）に置く — 一覧の1項目ではなく、一覧そのものが
// そこで打ち切られていることを表す行なので、階層の外側に出す
pub(crate) fn overflow_row(hidden: usize, above: bool, cols: usize) -> Line {
    let marker = if above { "▴" } else { "▾" };
    // `…` はペインの cwd（truncate_start）と同じ省略記号。マーカーと数字の
    // あいだに挟み、一覧がそこで途切れていることを添える。
    // **消さないこと** — 下端の `▾` はアクティブなタブ見出しと記号も列も同じなので、
    // この `…` だけが「タブ見出しではない」ことを示している
    let label = format!("{} … {} more", marker, hidden);
    // 一覧の行そのものではないので、通知行と同じく落として出す
    compose(&[(&label, Ink::Muted)], content_cols(cols))
}

// スクロール位置 `scroll` のとき、一覧を何行ぶん画面に出せるか。
// あふれマーカー行も画面の行を消費するので、その分を引く
fn rows_shown(list_len: usize, area: usize, scroll: usize) -> usize {
    let mut shown = area;
    if scroll > 0 {
        shown = shown.saturating_sub(1);
    }
    if scroll + shown < list_len {
        shown = shown.saturating_sub(1);
    }
    shown.min(list_len.saturating_sub(scroll))
}

// 選択行が画面に入るようスクロール位置を寄せ直す
//（.docs/issues/issue-sidebar-vertical-overflow.md）。行番号はいずれも
// 一覧（固定行を除いた部分）の中で数える。
//
// `anchor` は選択行の範囲（ペイン行 + cwd行のように2行にまたがる）。
// None のときは寄せずに範囲外への行き過ぎだけを直す
pub(crate) fn reconcile_scroll(
    list_len: usize,
    area: usize,
    scroll: usize,
    anchor: Option<(usize, usize)>,
) -> usize {
    // 全部載るならスクロールしない。ここを通さないと、一覧が減ったときに
    // 上へ寄ったままの表示が残る
    if area == 0 || list_len <= area {
        return 0;
    }
    let mut scroll = scroll.min(list_len - 1);
    // 末尾に余白を作らない位置まで戻す（一覧が縮んだあと）
    while scroll > 0 && scroll - 1 + rows_shown(list_len, area, scroll - 1) >= list_len {
        scroll -= 1;
    }
    if let Some((first, last)) = anchor {
        if first < scroll {
            // 上へ外れているなら選択行を先頭に置く
            scroll = first;
        } else {
            // 下へ外れているぶんだけ送る。マーカー行の有無で収容量が1行変わるので、
            // 1行ずつ送って入ったかを確かめる
            while scroll < list_len - 1 && last >= scroll + rows_shown(list_len, area, scroll) {
                scroll += 1;
            }
        }
    }
    // 上端マーカーが1行しか隠さないなら、マーカーではなくその行そのものを出す。
    // どちらも画面の1行を使うので、隠すほうが損（先頭タブの見出し行がこれに当たる）。
    // 収まる範囲の下端は変わらないので、選択行が押し出されることもない
    if scroll == 1 {
        return 0;
    }
    scroll
}
