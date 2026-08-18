// プレビュー（決定202608082045。要件: docs/requirements/preview/）。
//
// navモード中、いま光っている行のペインの内容を一時的なフローティングペインへ
// 出して、フォーカスを移さずに「そのペインで何が起きているか」を確かめる。
//
// 専用サブモードは作らない — マーク（決定202608080250）と同じトグルだけの横断的操作で、
// ツリー表示・検索の絞り込み結果・トリアージ一覧のどこでも同じキーで効く。
// 対象は「いま光っている行」（`highlighted_pane`）。
//
// **スナップショット方式。** 光っている行が動いたときに `get_pane_scrollback` を
// 1回だけ呼んで内容を差し替える。対象ペインが裏で動き続けても、行を動かさない
// 限り表示は変わらない（決定202608082045。ポーリングはしない）。
//
// 取れるのは装飾を持たないプレーンテキストだけで、色付きの忠実な画面再現を運ぶ
// 経路はプラグインには公開されていない（決定202608082045の技術調査）。yazi 水準の再現は
// 狙わず「大体の様子を掴む」用途に絞る。
//
// 描画は fujin 自身の別インスタンスが行う（臨時召喚と同じく `open_plugin_pane_floating`
// で自分のURLを開く）。プラグインは自分のペインの外を描けないため、
// 権威インスタンス（決定202608012142）がスナップショットを撮って pipe で送りつける形になる。

use std::collections::BTreeMap;

use zellij_tile::prelude::*;

use crate::config::PREVIEW_KEY;
use crate::summon::SIDEBAR_WIDTH;
use crate::{State, PREVIEW_PIPE};

// プレビュー用フローティングペインの左端。常駐サイドバー（幅32）の右端へ
// 隙間なく続ける — サイドバーの延長として一体に見せるため。**重ねはしない**
// （選択を動かしながら見るものなので、一覧が隠れると操作対象を見失う）。
// かつては +2 のオフセットを置いていたが、背景が透けて「ズレて浮いている」
// 見え方になっていた（docs/issues/preview-pane-and-keybind-swap.md）
const PREVIEW_X: usize = SIDEBAR_WIDTH;
// 幅は画面の1/3強。「大体の様子を掴む」（決定202608082045）には足りる一方、
// 半分取ると作業ペインの取り分が圧迫されて見えた（同上）
const PREVIEW_WIDTH_PERCENT: usize = 35;

// プレビューがオンのあいだ持つ状態。オン/オフそのものは `State::preview` の
// `is_some()` で表す（排他的なUI状態は Option で持つ、の指針どおり）
#[derive(Debug, Default)]
pub(crate) struct PreviewState {
    // 開いているプレビュー用フローティングペインのプラグインID。
    // 開けていない（自分のURLがまだ分からない等）あいだは None
    pub(crate) pane: Option<u32>,
    // 直近にスナップショットを送った対象ペイン。同じ行に留まっているあいだ
    // `get_pane_scrollback` を呼び直さないためのメモ
    pub(crate) target: Option<u32>,
}

// プレビュー用フローティングペインが描く内容。配られたものをそのまま持つだけで、
// このインスタンスは一覧もエージェント状態も持たない
#[derive(Debug, Default)]
pub(crate) struct PreviewContent {
    // 対象ペインの表示名。どのペインを見ているかは選択行のハイライトでも
    // 分かるが、プレビュー側だけを見ているときの手がかりとして出す
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

impl State {
    // プレビューのトグル。同じキーで開け閉めする（決定202608082045）
    pub(crate) fn toggle_preview(&mut self) {
        if self.preview.is_some() {
            self.close_preview();
            return;
        }
        self.preview = Some(PreviewState::default());
        self.refresh_preview();
    }

    // 表示内容を光っている行に追従させる。navモード内のキー操作のあとに毎回通す。
    //
    // **番号ジャンプサブモード中は更新しない**（決定202608082045）。候補を絞っている
    // あいだは対象ペインが定まらず（1件に確定した瞬間にジャンプするので選ぶ間が
    // 無い。決定202608070342）、直前の表示を維持するほうが筋が通る
    pub(crate) fn refresh_preview(&mut self) {
        if self.preview.is_none() || self.jump.is_some() {
            return;
        }
        // 開けていなければここで開く。トグルオンの直後と、開くのに失敗した
        // 次のフレームがこれに当たる
        self.open_preview_pane();
        let Some(target) = self.highlighted_pane() else {
            return;
        };
        if self.preview.as_ref().and_then(|preview| preview.target) == Some(target) {
            return;
        }
        if let Some(preview) = &mut self.preview {
            preview.target = Some(target);
        }
        self.push_preview_snapshot(target);
    }

    // プレビューを畳む。ジャンプ・navモードの退場でも必ずここを通す —
    // navモードの外にプレビューだけ残すのは決定202608072359（フォーカスの預かり）と
    // 整合しない
    pub(crate) fn close_preview(&mut self) {
        let Some(preview) = self.preview.take() else {
            return;
        };
        if let Some(pane_id) = preview.pane {
            close_plugin_pane(pane_id);
        }
    }

    // プレビュー中のペインを既読にする（決定202608082045）。プレビューはフォーカスも
    // キー横取りも対象ペインに及ぼさないので、見て回るだけでは既読にならない。
    // 「確認した」を明示的に伝えるのがこのキーの役目。
    //
    // **プレビューがオフの間は未定義キー扱い**で navモードごと退場する
    // （プレビュー専用のキーはプレビュー文脈の外では存在しない、の一貫性）
    pub(crate) fn mark_preview_read(&mut self) {
        if self.preview.is_none() {
            self.leave_nav_mode();
            return;
        }
        let Some(pane_id) = self.highlighted_pane() else {
            return;
        };
        self.read_pane(pane_id);
    }

    // 指定ペインを既読にして兄弟インスタンスへ配る（決定202607302302・決定202608012141）。
    // 注意を引く状態を持っていなければ何も起きない。
    //
    // コマンド状態は猶予（決定202608072218の `awaiting_refocus`）を見ない `force_read` を
    // 使う — 猶予は「フォーカスが通り過ぎただけで消さない」ための仕掛けで、
    // ユーザーが自分でキーを押した明示的な既読化には掛からない
    fn read_pane(&mut self, pane_id: u32) -> bool {
        let mut cleared = false;
        if let Some(agent) = self.agents.get_mut(&pane_id) {
            cleared |= agent.mark_read();
        }
        if let Some(info) = self.commands.get_mut(&pane_id) {
            cleared |= info.force_read();
        }
        if cleared {
            self.broadcast_read_clears(&[pane_id]);
        }
        cleared
    }

    // プレビュー用フローティングペインを開く。既に開いていれば何もしない。
    //
    // 臨時召喚（決定202608011644）と同じく自分のURLを configuration 付きで開く。
    // 開いた側（＝権威インスタンス）にはフォーカスが残らない（実測では
    // 新しく開いたペインへ移る）ので、預かったフォーカス（決定202608072359）を
    // 取り返しておく
    fn open_preview_pane(&mut self) {
        if self
            .preview
            .as_ref()
            .is_none_or(|preview| preview.pane.is_some())
        {
            return;
        }
        let Some(own_url) = self.own_plugin_url.clone() else {
            eprintln!("fujin: preview skipped (own url unknown)");
            return;
        };
        let mut config = BTreeMap::new();
        config.insert(PREVIEW_KEY.to_string(), "true".to_string());
        let opened = open_plugin_pane_floating(
            &own_url,
            config,
            Some(Self::preview_coordinates()),
            BTreeMap::new(),
        );
        let Some(PaneId::Plugin(pane_id)) = opened else {
            eprintln!("fujin: preview failed (no pane id returned)");
            return;
        };
        // 開くときに渡した座標は効かず、pinned だけが通る（召喚で実測済み）。
        // 開いた後に指定し直す
        change_floating_panes_coordinates(vec![(
            PaneId::Plugin(pane_id),
            Self::preview_coordinates(),
        )]);
        // フォーカスをサイドバーへ戻す。戻さないとフォーカス枠がプレビュー側に
        // 点いたままになる（navモード中のフォーカスはサイドバーが預かっている、
        // 決定202608072359）。**退場の判定はこの取り返しを待たない** — `refresh_focus` は
        // プレビュー用フローティングペインを自分の一部として数えるので、
        // 移動が届く前に PaneUpdate が来ても navモードを抜けたりはしない
        if let Some(own_id) = self.own_plugin_id {
            focus_plugin_pane(own_id, false, false);
        }
        if let Some(preview) = &mut self.preview {
            preview.pane = Some(pane_id);
        }
    }

    // 対象ペインの内容を1回だけ撮ってプレビュー用フローティングペインへ送る。
    //
    // `get_full_scrollback` は使わない（決定202608082045）。欲しいのは「いま画面に見えて
    // いる範囲」で、遡れる全部ではない
    fn push_preview_snapshot(&self, target: u32) {
        let Some(pane_id) = self.preview.as_ref().and_then(|preview| preview.pane) else {
            return;
        };
        let title = self.preview_title(target);
        let body = match get_pane_scrollback(PaneId::Terminal(target), false) {
            Ok(contents) => contents.viewport.join("\n"),
            Err(error) => {
                eprintln!("fujin: preview snapshot failed for pane {target}: {error}");
                // 空のまま出すと「静かなペイン」と見分けが付かないので、
                // 取れなかったことを本文として出す
                UNAVAILABLE.to_string()
            }
        };
        crate::sync::send_to_plugin(pane_id, PREVIEW_PIPE, format!("{title}\n{body}"));
    }

    // プレビューの見出しに出す対象ペインの名前。一覧から落ちていたら空にする
    //（対象が閉じられた直後に起こりうる）
    fn preview_title(&self, target: u32) -> String {
        self.selectable
            .iter()
            .find(|entry| entry.pane_id == target)
            .map(|entry| self.display_title(entry).to_string())
            .unwrap_or_default()
    }

    // fujin_preview の受け口。描き手（プレビュー用フローティングペイン）だけが
    // 受け取る。宛先はプラグインIDで指定しているので他へは飛ばないが、
    // 取り違えても描くものが無いだけで済むよう役割で弾いておく
    pub(crate) fn handle_preview_pipe(&mut self, payload: Option<&str>) -> bool {
        if !self.is_preview {
            return false;
        }
        payload
            .map(|raw| self.apply_preview_snapshot(raw))
            .unwrap_or(false)
    }

    // 配られたスナップショットを取り込む（プレビュー用フローティングペイン側）。
    // 1行目が対象ペイン名、2行目以降が内容
    pub(crate) fn apply_preview_snapshot(&mut self, raw: &str) -> bool {
        let (title, body) = raw.split_once('\n').unwrap_or((raw, ""));
        self.preview_content = PreviewContent {
            title: title.to_string(),
            lines: body.lines().map(str::to_string).collect(),
        };
        true
    }

    // 指定プラグインIDが自分の開いたプレビュー用フローティングペインか。
    // フォーカスの判定（決定202608072359）が自分の一部として扱うために要る
    pub(crate) fn is_preview_pane(&self, plugin_id: u32) -> bool {
        self.preview
            .as_ref()
            .and_then(|preview| preview.pane)
            .is_some_and(|id| id == plugin_id)
    }

    // プレビュー用フローティングペインの配置。常駐サイドバーの右隣に、
    // 画面の高さいっぱいの縦長で置く。
    //
    // ピン留めは召喚と同じ理由で必須（タブのフローティング層は既定で非表示の
    // ため、ピン留めしないとペインは在るのに描画されない）
    fn preview_coordinates() -> FloatingPaneCoordinates {
        let mut coordinates = FloatingPaneCoordinates::default()
            .with_x_fixed(PREVIEW_X)
            .with_y_fixed(0)
            .with_width_percent(PREVIEW_WIDTH_PERCENT)
            .with_height_percent(100);
        coordinates.pinned = Some(true);
        coordinates
    }

    // 表示できる高さに収めたスナップショットの行（プレビュー用フローティング
    // ペイン側の描画用）。
    //
    // **末尾を優先して残す。** 直近の出力（エラーが出ていないか、止まっていないか）
    // こそが見たいものなので、溢れるときに落とすのは古い側。末尾の空行は先に
    // 捨てる — プロンプトの下の余白がそのまま入ってくると、画面いっぱいの空行を
    // 見せて終わることがある
    pub(crate) fn preview_body(&self, area: usize) -> &[String] {
        let lines = &self.preview_content.lines;
        let end = lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map(|index| index + 1)
            .unwrap_or(0);
        let start = end.saturating_sub(area);
        &lines[start..end]
    }
}

// スナップショットを取れなかったときに本文の代わりに出す文言。
// UI文言は英語で統一する（ui-design.md）
pub(crate) const UNAVAILABLE: &str = "(scrollback unavailable)";
