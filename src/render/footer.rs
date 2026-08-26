// フッター行（決定202608070119。要件: features/sidebar-tree/sidebar-footer.feature）。
//
// **いまその状態で使えるコマンドを常に出し、空にはしない**。入力欄を兼ねる状態
// （検索サブモードのクエリ・番号ジャンプサブモードの番号入力）と、終了操作の確認
// プロンプト・設定の警告もここが出し分ける。

use super::*;

// 入力欄（検索・番号ジャンプ）の状態ごとの見た目。
//
// **疑似カーソルは持たない**（決定202608131200、2026-08-13 に地の文の疑似カーソルを廃止。
// 経緯: .docs/issues/issue-search-input-cursor-shape.md）。位置表示は編集状態・操作状態
// どちらもテキストカーソル（`sync_input_cursor`）に一本化した——プラグイン側から形状を
// 指定できず端末既定はほぼブロックなので、地の文の字を描き分けてもテキストカーソルの
// 下に隠れる/隠れないでコロコロ変わり、当てにならない見分け手段だった。
// 状態を見分ける主手がかりは入力文字列の明暗（InputStyle::ink）とフッターの
// ヒント文言の2つ
#[derive(Clone, Copy)]
struct InputStyle {
    // 入力文字列の色。打てる状態は本文のまま、打てない状態は dim
    ink: Ink,
    hints: &'static [&'static str],
}

// 番号ジャンプサブモードは常に打てる状態しかない（決定202608070342）
const JUMP_INPUT: InputStyle = InputStyle {
    ink: Ink::Plain,
    hints: &["?:help"],
};

// 検索サブモードの入力欄の見た目（決定202608131200。要件: nav-mode-hints）。
// **編集状態では `?:help` を出さない** — そこでの `?` はクエリの文字になるので、
// 押せば助けが出ると読める案内は嘘になる。
// 幅に収まらないヒントは末尾から落ちる（fit_hint）ので、重要な順に並べる
fn search_input_style(phase: SearchPhase) -> InputStyle {
    match phase {
        SearchPhase::Editing => InputStyle {
            ink: Ink::Plain,
            hints: &["esc:browse", "enter:jump"],
        },
        SearchPhase::Navigating => InputStyle {
            ink: Ink::Muted,
            hints: &["j/k:move", "?:help", "i:edit", "esc:cancel", "enter:jump"],
        },
    }
}

impl State {
    // フッター（1行）。**いまその状態で使えるコマンドを常に出し、空にはしない**
    //（決定202608070119。要件: sidebar-footer）。
    //
    // サイドバー幅は32文字（決定202607302256）で全キーの説明は載らないので、常時出すのは
    // ヘルプ・退出キーだけに絞り、詳細は `?` のヘルプオーバーレイへ追い出して
    // ある。文言は英語で統一する。
    //
    // 文字色は状態色で統一する — 「キーは常にレベル2固定」という色役割の原則は
    // フッターに限り例外（決定202608070119）。ヘッダーの三角とトーンを揃えるほうを取る
    pub(crate) fn footer_line(&self, cols: usize) -> Line {
        // ツリーの行と同じく右端は空ける
        let inner = content_cols(cols);
        let indent = " ".repeat(HEADER_INDENT);
        let ink = self.state_ink();
        // ヘルプオーバーレイ中は閉じ方だけを出す。ヘルプ本文の末尾にあった
        // 同じ文言はこちらへ移してある（本文側からは削除済み）
        if self.help_overlay {
            return compose(
                &[(&indent, Ink::Plain), ("press any key to close", ink)],
                inner,
            );
        }
        // 終了操作サブモード中は確認プロンプトに転用する（決定202608080140。要件:
        // pane-close-kill）。**ペイン名は出さない** — 対象は選択行のハイライトで
        // 分かっており、この幅では名前の大半が切り詰められて識別の役に立たない
        if self.termination.is_some() {
            // マークが1件以上あれば件数を前置する（決定202608080250）
            let prompt = termination_prompt(
                self.termination_marked_count(),
                inner.saturating_sub(HEADER_INDENT),
            );
            return compose(&[(&indent, Ink::Plain), (&prompt, ink)], inner);
        }
        // 検索サブモード中はクエリ入力欄に転用する。クエリの明暗と操作ヒントは
        // 編集状態/操作状態で出し分ける（決定202608131200。要件: nav-mode-hints）
        if let Some(search) = &self.search {
            let style = search_input_style(search.phase);
            return input_footer("/", &search.query, style, &indent, ink, inner);
        }
        // 番号ジャンプサブモード中は番号入力バッファの表示に転用する（決定202608070342）。
        // 先頭の `n` は検索サブモードの `/` と同じく入場キーの提示
        if let Some(jump) = &self.jump {
            return input_footer("n ", &jump.buffer, JUMP_INPUT, &indent, ink, inner);
        }
        // 解釈できなかった設定の警告（決定202608080346）。**入力欄・確認プロンプト・
        // ヘルプより後、静的なヒントより先**に見る（`showing_config_warning`）。
        // 起動直後の一定時間だけで、期限が切れれば通常の表示へ戻る
        if self.showing_config_warning() {
            let warning = self.config_warning_line(inner.saturating_sub(HEADER_INDENT));
            return compose(&[(&indent, Ink::Plain), (&warning, ink)], inner);
        }
        // トリアージモードの Esc の行き先は navモードのツリー表示（退場ではない）
        // なので、ヒントも `exit` ではなく `back`
        if self.triage.is_some() {
            return compose(&[(&indent, Ink::Plain), ("?:help  esc:back", ink)], inner);
        }
        if self.nav_mode {
            return compose(&[(&indent, Ink::Plain), ("?:help  esc:exit", ink)], inner);
        }
        // 非フォーカス時は direct-keys方式（決定202607302258）のヒント
        compose(
            &[
                (&indent, Ink::Plain),
                (
                    &self.direct_keys_hint(inner.saturating_sub(HEADER_INDENT)),
                    ink,
                ),
            ],
            inner,
        )
    }

    // 非フォーカス時のフッターに出す direct-keys のヒント（要件: sidebar-footer）。
    //
    // キーは決め打ちせず、`Event::InitialKeybinds` から解決した実際の割り当てを
    // 出す（決定202608070119）。未バインドの項目はその項目だけ省く。
    //
    // 実キーの長さはユーザー依存で伸び縮みするので、幅に収まらないときは
    //  1. `up`/`down` を矢印へ落とす（`jump`/`cwd` に対応する矢印記号は無い）
    //  2. それでも溢れるなら末尾の項目ごと省く
    // の順で削る。**末尾を `…` で切り詰めない** — `キー:動作` の形が壊れた
    // ヒントは読めず、項目ごと省いたほうが残りは正しく読める。
    // 項目数を優先し、同じ項目数で選べるなら既定の英字表記を採る。
    //
    // 修飾キーのまとめ（`join_direct_keys`）は幅対策ではなく常時適用の表示ルール
    // なので、**組んだ後の1行に対して**上の削り方をそのまま当てる。まとめと幅対策の
    // 優先順位を競わせない
    fn direct_keys_hint(&self, budget: usize) -> String {
        let full = self.direct_key_parts(false).len();
        for count in (1..=full).rev() {
            for arrows in [false, true] {
                let mut parts = self.direct_key_parts(arrows);
                parts.truncate(count);
                let line = join_direct_keys(&parts);
                if UnicodeWidthStr::width(line.as_str()) <= budget {
                    return line;
                }
            }
        }
        String::new()
    }

    // 割り当てのある項目だけを「キー表記と動作名」の組にしたもの（表示順）。
    // 並び順も動作名も設定テーブル（config.rs）から引く（決定202608080346）
    fn direct_key_parts(&self, arrows: bool) -> Vec<(&str, &'static str)> {
        let mut parts = Vec::new();
        for setting in &SETTINGS {
            let Kind::DirectKey { label, arrow } = setting.kind else {
                continue;
            };
            let Some(key) = self.direct_keys.get(setting.key) else {
                continue;
            };
            let label = match (arrows, arrow) {
                (true, Some(arrow)) => arrow,
                _ => label,
            };
            parts.push((key.as_str(), label));
        }
        parts
    }

    // 解釈できなかった設定を伝える1行（決定202608080346。要件: configuration）。
    //
    // 幅32のフッターに理由まで書く余地は無いので、**どのキーが効いていないか**
    // だけを出して、直す場所を指させる形に絞る。詳細は README の設定表と
    // stderr のログ側にある。項目が幅に収まらないときは、direct-keys のヒントと
    // 同じ考え方で末尾を `…` で切らず、残り件数（`+N`）に畳む
    fn config_warning_line(&self, budget: usize) -> String {
        let keys = &self.config_warnings;
        let head = if keys.len() == 1 {
            "!bad value: "
        } else {
            "!bad values: "
        };
        for shown in (1..=keys.len()).rev() {
            let mut line = format!("{}{}", head, keys[..shown].join(" "));
            if shown < keys.len() {
                line.push_str(&format!(" +{}", keys.len() - shown));
            }
            if UnicodeWidthStr::width(line.as_str()) <= budget {
                return line;
            }
        }
        // キー名が1つも置けない幅。せめて「設定を見ろ」だけは残す
        "!bad config".to_string()
    }
}

// 修飾キーをまとめた表記の区切り（要件: sidebar-footer）。**新しい記号を増やさず、
// 未起動マーカー（`NO_AGENT_ICON`）と同じ字を流用する** — 出る場所が状態アイコン列と
// フッターで別なので、同じ行に並んで混同することがない
// （.docs/issues/issue-direct-keys-hint-modifier-prefix.md）
const MODIFIER_GROUP_MARK: &str = NO_AGENT_ICON;

// direct-keys の項目を1行に組む（要件: sidebar-footer）。
//
// 全項目が同じ修飾キーを持つなら、項目ごとに繰り返される修飾キーを先頭へまとめて
// `alt + › u:up  d:down` の形にする（zellij本体の status-bar に寄せた見た目）。
// まとまらないときは既存の詰めた表記（`alt+u:up  ctrl+g:jump`）へ**丸ごと**落とす
// — 部分的にまとめると、どの項目にどの修飾キーが要るのかが読めなくなる。
// 単項目の綴り `alt+u` は決定202608070226・README で定着しているので、キー表記の
// 正本（`format_key`）自体には手を入れない
fn join_direct_keys(parts: &[(&str, &'static str)]) -> String {
    let Some(modifiers) = shared_modifiers(parts) else {
        return parts
            .iter()
            .map(|(key, label)| format!("{}:{}", key, label))
            .collect::<Vec<_>>()
            .join("  ");
    };
    let items = parts
        .iter()
        .map(|(key, label)| {
            let bare = split_modifiers(key).map_or(*key, |(_, bare)| bare);
            format!("{}:{}", bare, label)
        })
        .collect::<Vec<_>>()
        .join("  ");
    format!("{} + {} {}", modifiers, MODIFIER_GROUP_MARK, items)
}

// 全項目に共通する修飾キー列。まとめられないなら `None`。条件は3つ:
//  - 項目が2つ以上ある（1つでは繰り返しが無く、`alt+u` のほうが短く済む）
//  - 全項目が修飾キーを持つ（`pgup` のような無修飾が1つでも混ざれば落とす）
//  - 修飾キー列が**完全一致**する（`ctrl+g` と `ctrl+shift+p` はまとめない —
//    先頭だけの一致でまとめると `shift` の要不要が消えて誤操作を招く）
fn shared_modifiers<'a>(parts: &[(&'a str, &'static str)]) -> Option<&'a str> {
    if parts.len() < 2 {
        return None;
    }
    let (first, _) = parts.first()?;
    let (modifiers, _) = split_modifiers(first)?;
    parts
        .iter()
        .all(|(key, _)| split_modifiers(key).map(|(m, _)| m) == Some(modifiers))
        .then_some(modifiers)
}

// `alt+u` を修飾キー列 `alt` と素のキー `u` に分ける（`format_key` が組んだ表記が
// 前提。config.rs）。無修飾なら `None`。**末尾の1文字は必ず素のキー側に残す** —
// `alt++`（`+` そのものへの割り当て）で区切りを取り違えないため
fn split_modifiers(key: &str) -> Option<(&str, &str)> {
    let (last, _) = key.char_indices().next_back()?;
    let at = key[..last].rfind('+')?;
    Some((&key[..at], &key[at + 1..]))
}

// 入力欄を兼ねるフッター（検索サブモードのクエリ・番号ジャンプサブモードの
// 番号入力バッファ）。入力が主役なので左に置き、操作ヒントは右端へ寄せる。
// 入力が伸びてぶつかるところまで来たら、入力中の文字列のほうを優先して
// ヒント側を落とす（右寄せを使うのは枠でここ1箇所だけ）。
//
// `hints` は右端に出す操作ヒントの項目で、収まらないぶんは末尾から落とす。
// 入力位置はテキストカーソル（`sync_input_cursor`）が示す。地の文の疑似カーソルは
// 持たない（決定202608131200。経緯: .docs/issues/issue-search-input-cursor-shape.md）
//
// 入力本体は本文なので既定色のまま、先頭の `tag`（`/` や `n`）はモード名と
// 同じ扱いでレベル3。状態色で統一するのはヒント側（要件: sidebar-footer）
fn input_footer(
    tag: &str,
    input: &str,
    style: InputStyle,
    indent: &str,
    ink: Ink,
    cols: usize,
) -> Line {
    // 余白は表示セル幅で数える。入力に全角文字が入ると文字数とセル数が
    // ずれ、操作ヒントが右端からはみ出す
    let budget = cols
        .saturating_sub(UnicodeWidthStr::width(indent))
        .saturating_sub(UnicodeWidthStr::width(tag))
        .saturating_sub(UnicodeWidthStr::width(input));
    let hint = fit_hint(style.hints, budget);
    let pad = budget.saturating_sub(UnicodeWidthStr::width(hint.as_str()));
    let mut segments = vec![(indent, Ink::Plain), (tag, Ink::Tag), (input, style.ink)];
    let spacer = " ".repeat(pad);
    if !hint.is_empty() {
        segments.push((spacer.as_str(), Ink::Plain));
        segments.push((hint.as_str(), ink));
    }
    compose(&segments, cols)
}

// 入力欄の右に出せるだけのヒント。**末尾の項目ごと落とす** — `キー:動作` の形が
// 壊れたヒントは読めないので `…` で切らない（direct-keys のヒントと同じ削り方。
// .docs/issues/issue-direct-keys-hint-overflow.md）。入力とヒントの間は最低1セル空ける
fn fit_hint(hints: &[&str], budget: usize) -> String {
    for count in (1..=hints.len()).rev() {
        let line = hints[..count].join("  ");
        if UnicodeWidthStr::width(line.as_str()) < budget {
            return line;
        }
    }
    String::new()
}

// 終了操作サブモードの確認プロンプト（決定202608080140。要件: pane-close-kill）。
//
// 幅28セル（決定202608070119）に3項目とも収める都合で、項目のあいだは他のヒントの半分の
// 空白1つ。それでも収まらない幅ではキーだけに落とす — 項目の途中で切り詰めると
// `キー:動作` の形が壊れて読めなくなる（direct-keys のヒントと同じ考え方）。
//
// `marked` が1件以上なら件数を前置する（決定202608080250）。**幅が足りないときも件数だけは
// 残す** — 何件消えるかは対象の数が1件のときと違って行のハイライトから読めず、
// キーの意味（`c k x` の3択）より先に知りたい情報だから。マーク0件（選択行への
// フォールバック）では単一版と同じ文言のままにする。
//
// Esc（取り消し）はここには出ない。3項目で幅を使い切っているので、行き先の説明は
// ヘルプオーバーレイ側（termination_help_lines）が担う
fn termination_prompt(marked: usize, budget: usize) -> String {
    let count = match marked {
        0 => String::new(),
        1 => "1 pane  ".to_string(),
        n => format!("{} panes  ", n),
    };
    let budget = budget.saturating_sub(UnicodeWidthStr::width(count.as_str()));
    let full = "c:close k:kill x:kill+close";
    if UnicodeWidthStr::width(full) <= budget {
        format!("{}{}", count, full)
    } else {
        format!("{}c k x", count)
    }
}
