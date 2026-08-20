// フォーカス同期（用語: docs/terms/focus-sync.md。要件: docs/requirements/req-focus-sync.md）と
// フォーカスの預かり（決定202608072359。用語: docs/terms/focus-parking.md）。
//
// 前者は「zellij の実フォーカスを観測して選択へ取り込む」、後者は「navモードの
// 入場から退場までのあいだ実フォーカスをサイドバー自身へ移し、退場で作業ペインへ返す」。
// 実フォーカスを軸にした表裏なので同じモジュールに置く。
//
// ここは `get_focused_pane_info()` を呼ぶためユニットテストで守れない範囲を含む
// （docs/dev/build-and-test.md「テストで検証できない範囲」）。触ったら実機確認が要る。

use zellij_tile::prelude::*;

use crate::host;
use crate::State;

// navモード中、サイドバーへ預けたフォーカスの戻し先（決定202608072359）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ParkedFocus {
    pub(crate) pane_id: u32,
    // 戻すときの should_float_if_hidden に使う（`focus_selected` と同じ理由）
    pub(crate) is_floating: bool,
    // 預かったフォーカスが実際に自分へ来たのを一度でも観測したか。
    // フォーカス移動は非同期なので、待たずに判定すると入場直後に退場してしまう
    //（`park_taken_over`）
    pub(crate) confirmed: bool,
}

impl State {
    // 自分が可視インスタンスか（決定202608012142の権威判定のうち、**副作用のない部分だけ**）。
    //
    // `refresh_focus()` を流用しないのは、あちらが選択の追従・navモード退場という
    // 副作用を持つため — 状態通知が届いただけで探索位置を動かすわけにはいかない。
    // `self.visible` を第一手にしないのも refresh_focus と同じ理由（リロードで
    // `Event::Visible` が再送されない）。問い合わせに失敗したときだけ旗に落ちる
    pub(crate) fn is_visible_instance(&self) -> bool {
        let Ok((focused_tab, _)) = get_focused_pane_info() else {
            return self.visible;
        };
        self.owns_tab(focused_tab)
    }

    // フォーカス情報をサーバへ1回だけ問い合わせて、
    //  - 観測したフォーカスを取り込み、navモード外なら選択行を追従させる
    //    （要件: docs/requirements/req-focus-sync.md）
    //  - 自分が操作の権威を持つインスタンスか（決定202608012142）を返す
    //
    // イベントの配送は権威判定に当てにできない — PaneUpdate / TabUpdate は
    // 非可視インスタンスに届かず「自分のタブがアクティブ」が複数現れ（実測）、
    // Event::Visible はリロードで再送されない。get_focused_pane_info() への
    // 直接問い合わせなら常に最新で、権威になるインスタンスは1つだけになる
    pub(crate) fn refresh_focus(&mut self) -> bool {
        let Ok((focused_tab, focused_pane)) = get_focused_pane_info() else {
            // 問い合わせに失敗したときだけ Visible に落とす
            return self.visible;
        };
        if !self.owns_tab(focused_tab) {
            // フォーカス中のタブに居ないインスタンスは選択を自分では動かさない。
            // 動かすのは権威1つだけで、兄弟へは決定202608012141の同期で配られる
            return false;
        }
        self.focus_on_terminal = matches!(focused_pane, PaneId::Terminal(_));
        // プレビュー用フローティングペインは自分の一部として数える（決定202608082045）。
        // 別ペイン扱いにすると、開いた直後の移動を「持って行かれた」と誤読して
        // navモードを抜けてしまう
        let own_pane_focused = match (focused_pane, self.own_plugin_id) {
            (PaneId::Plugin(id), Some(own_id)) => id == own_id || self.is_preview_pane(id),
            _ => false,
        };
        // 預かりが成立したのを観測しておく（決定202608072359。`park_taken_over` が使う）
        if own_pane_focused {
            if let Some(parked) = self.focus_parked.as_mut() {
                parked.confirmed = true;
            }
        }
        // 預けたフォーカスをユーザーの操作で持って行かれたら、奪い返さずに手放す
        let park_lost = self.park_taken_over(own_pane_focused);
        if park_lost {
            self.release_parked_focus(false);
        }
        let focused = match focused_pane {
            PaneId::Terminal(id) => Some(id),
            // フォーカスを預かっている間（決定202608072359）は預かった当のペインを指す。
            // 一覧から拾い直すと「タイル層でフォーカス中のターミナル」が居らず
            // None になり、探索位置も戻し先も失う
            PaneId::Plugin(_) if own_pane_focused && self.focus_parked.is_some() => self
                .focus_parked
                .map(|parked| parked.pane_id)
                .or_else(|| self.focused_terminal_in_tab(focused_tab)),
            // 他のプラグインペインは selectable に無いので、一覧から作業ペインを
            // 拾い直す（召喚インスタンス自身がフォーカスを持つ場合がこれ）
            PaneId::Plugin(_) => self.focused_terminal_in_tab(focused_tab),
        };
        // 可視化直後の1回は、キャッシュと同じフォーカスでも引き直す
        let force = std::mem::take(&mut self.pending_focus_resync);
        let follow = self.focus_to_follow(focused, force);
        // navモード中に実フォーカスが動いた＝探索をやめて作業に戻ったとみなして
        // 退場する（要件: nav-mode / focus-sync）。主な経路はマウスでのペイン選択で、
        // 横取りを解かないと移動先で j/k が食われ続ける。預けたフォーカスを
        // 持って行かれた場合（`park_lost`）も同じ扱い — 行き先が預かる前と同じ
        // ペインでも「作業に戻る」なので、ペインIDの比較だけでは取りこぼす
        let interrupted =
            self.nav_mode && (park_lost || (focused.is_some() && focused != self.focused_pane));
        // 記録は追従しない場合も続ける。navモード退場時の「動いたか」の比較材料
        self.focused_pane = focused;
        if interrupted {
            // 預かりの手放しは上（`park_lost`）で済んでいる。ここで返す形にすると
            // ユーザーが自分で選んだ先からフォーカスを奪い返してしまう
            self.leave_nav_mode();
        } else if let Some(pane_id) = follow {
            if self.select_pane_id(pane_id) {
                self.broadcast_selection();
            }
        }
        true
    }

    // 選択を引き直す先（要件: docs/requirements/req-focus-sync.md）。
    // `force` は可視化直後など、キャッシュを信用できないときに立てる
    pub(crate) fn focus_to_follow(&self, focused: Option<u32>, force: bool) -> Option<u32> {
        // navモード中の選択はユーザーの探索位置なので追従させない
        if self.nav_mode {
            return None;
        }
        let pane_id = focused?;
        // **フォーカスが動いたときだけ**引き直す。PaneUpdate はペイン名の変化
        // でも飛んでくるので、毎回引き直すと fujin_up / fujin_down で動かした
        // 選択が勝手に戻ってしまう
        if !force && focused == self.focused_pane {
            return None;
        }
        Some(pane_id)
    }

    // 指定タブでフォーカス中のターミナルペイン（要件: focus-sync）。
    //
    // `get_focused_pane_info()` がプラグインペインを返したときの受け皿。
    // 召喚インスタンス（臨時召喚・コールドスタートの両経路。決定202608011644）は**自分が
    // フローティング層のフォーカスを持つ**ため、問い合わせでは作業ペインが
    // 分からず、追従も入場時の初期選択も効かなくなる（実測）。
    //
    // `PaneInfo.is_focused` は**レイヤごと**の意味（`data.rs:2302`
    // "focused in its layer"）なので、フローティングが前面にあってもタイル層の
    // フォーカスは一覧に残っている。作業ペインは通常タイルなのでそちらを優先し、
    // 無ければフローティングのターミナルを拾う
    pub(crate) fn focused_terminal_in_tab(&self, tab_position: usize) -> Option<u32> {
        let panes = self.panes.as_ref()?.panes.get(&tab_position)?;
        let mut floating = None;
        for pane in panes {
            if pane.is_plugin || pane.is_suppressed || !pane.is_focused {
                continue;
            }
            if !pane.is_floating {
                return Some(pane.id);
            }
            floating = Some(pane.id);
        }
        floating
    }

    // 預けたフォーカスをユーザーの操作で持って行かれたか（決定202608072359）。
    // `own_pane_focused` は「いま実フォーカスが自分のペインにあるか」の観測結果。
    //
    // **`confirmed` を待つのが要点。** フォーカスの移動は非同期なので、
    // 預けた命令が処理される前に届いた `PaneUpdate` では自分にフォーカスが無い。
    // 確認を待たずに判定すると、入場した直後に「持って行かれた」と誤読して退場する
    pub(crate) fn park_taken_over(&self, own_pane_focused: bool) -> bool {
        self.focus_parked.is_some_and(|parked| parked.confirmed) && !own_pane_focused
    }

    // 指定ターミナルペインがフローティングか（決定202608072359）。フォーカスを預かるとき、
    // 戻すための `should_float_if_hidden` を控えておくのに使う。
    // 一覧に無ければ false ＝ タイル扱い（`focus_selected` の既定と同じ）
    pub(crate) fn pane_is_floating(&self, pane_id: u32) -> bool {
        self.selectable
            .iter()
            .find(|entry| entry.pane_id == pane_id)
            .map(|entry| entry.is_floating)
            .unwrap_or(false)
    }

    // navモード中だけ、実フォーカスをサイドバー自身へ預かる（決定202608072359）。
    //
    // フォーカス枠の色だけを消すAPIは無いが、**枠はセッション内で1枚しか点かない**
    //（実測）ので、フォーカスをサイドバーへ移せば作業ペインの枠は非フォーカス色に
    // 戻り、サイドバーのハイライトと二重に「ここが操作対象」を主張しなくなる。
    // 召喚インスタンス（決定202608011644）は最初から自分がフォーカスを持っているので、
    // これは常駐サイドバーを召喚と同じ状態に揃える操作でもある
    pub(crate) fn park_focus(&mut self) {
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
        // 巡回でサイドバーへ入り込む余地は無く、決定202607302258の意図は保たれる
        host::set_selectable(true);
        host::focus_plugin_pane(own_id, false, false);
        self.focus_parked = Some(ParkedFocus {
            pane_id,
            is_floating,
            confirmed: false,
        });
    }

    // 預かったフォーカスを手放す（決定202608072359）。`refocus` が真なら作業ペインへ返す。
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
                host::focus_pane_with_id(PaneId::Terminal(pane_id), is_floating, false);
            }
        }
        // 決定202607302258へ戻す。**フォーカスを返したあとに** unselectable にすること —
        // 逆順だと自分にフォーカスが残ったまま巡回対象から外れる
        host::set_selectable(false);
    }

    // フォーカスの返し先。原則は預かった当のペインだが、**navモード中に
    // 閉じられていたら選択行のペインへ返す**（決定202608072359）。unselectable な自分に
    // フォーカスが残ると、横取りも解いた後なのでキーの行き先が無くなる
    pub(crate) fn refocus_target(&self, parked: ParkedFocus) -> Option<(u32, bool)> {
        if self.selectable.iter().any(|e| e.pane_id == parked.pane_id) {
            return Some((parked.pane_id, parked.is_floating));
        }
        self.selectable
            .get(self.selected)
            .map(|entry| (entry.pane_id, entry.is_floating))
    }
}
