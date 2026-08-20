// 単体テストの共有ヘルパ（`src/tests/` 配下の各ファイルと、インライン化した
// `mod tests` から `crate::test_support::*` で参照する）。
//
// 実行はホストターゲットで行う（`make test`）。既定ターゲットの wasm32-wasip1 では
// テストバイナリを走らせるランタイムがないため。
//
// テストできる範囲について:
// - 副作用だけのホストコマンド（focus_pane_with_id, pipe_message_to_plugin,
//   intercept_key_presses 等）は host.rs の間接層がテストビルドで記録に差し替える
//   ため、`host::take_host_calls()` で発行と引数を検証できる
//  （docs/issues/issue-host-command-recorder.md）
// - **戻り値を stdin から読み返す問い合わせ系は呼べない**（get_plugin_ids,
//   get_focused_pane_info 等）。テスト中に呼ぶと stdin の読み取りに失敗して panic する。
//   したがって refresh_focus() とそれを経由する pipe ハンドラ（NAV_*）は
//   テストでは検証しない。フォーカス同期（要件: focus-sync）のうち、
//   問い合わせ結果を畳んだ先（State::focused_pane）から先のロジックは
//   フィールドを直接立てて検証する
// - navモードの入場は2経路を使い分ける。入場・退場の**副作用そのもの**
//   （park_focus・intercept_key_presses・初期選択の nav_entry_selection）を
//   検証するテストは本番経路の `state.enter_nav_mode()` を、navモード中の
//   **表示・キー解釈**だけを見るテストは `nav_mode = true` の直接代入
//   （または searchable_state 等のヘルパ）を使う。直接代入は focus_parked を
//   持たない最小状態を作る意図で、入場時の挙動に関わるテストを直接代入で
//   書くと nav_entry_selection まわりの回帰をすり抜ける
//  （docs/issues/issue-test-support-conventions.md）

use crate::agent::{AgentState, StatusPayload};
use crate::render::{CounterColumn, HeadCells, Row};
use crate::search::SearchPhase;
use crate::*;
use std::collections::HashMap;

// zellij-tile の shim は wasm ホストが提供する `host_run_plugin_command` を参照する。
// ホスト向けにリンクするにはこのシンボルを埋めてやる必要がある。
// **テストバイナリ全体でちょうど1回だけ**定義されればよいので、ここにだけ置く
//（分割先の各ファイルに複製するとシンボル重複でリンクが落ちる）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}

// --- ペイン・タブ・状態の組み立て ---

pub(crate) fn terminal_pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id,
        title: title.to_string(),
        ..Default::default()
    }
}

pub(crate) fn plugin_pane(id: u32, url: &str) -> PaneInfo {
    PaneInfo {
        id,
        is_plugin: true,
        plugin_url: Some(url.to_string()),
        ..Default::default()
    }
}

// フォーカスしたまま滞在猶予（READ_DELAY）が満ちるまで居座る
//（docs/issues/issue-transit-focus-clears-read-state.md）。
//
// 実機では 0.15 秒刻みで Timer が届くが、期限は経過時間で見るので
// 1回にまとめてよい。**目的地としてフォーカスした**ことの表明として、
// 既読を期待するテストはこれを挟む
pub(crate) fn settle_read(state: &mut State) {
    state.elapsed += READ_DELAY;
    state.apply_pending_reads();
}

// 召喚インスタンス（決定202608011644）。常駐との違いはフローティングかどうか
pub(crate) fn floating_plugin_pane(id: u32, url: &str) -> PaneInfo {
    PaneInfo {
        is_floating: true,
        ..plugin_pane(id, url)
    }
}

pub(crate) fn manifest(tabs: Vec<(usize, Vec<PaneInfo>)>) -> PaneManifest {
    PaneManifest {
        panes: tabs.into_iter().collect::<HashMap<_, _>>(),
    }
}

pub(crate) fn tab(position: usize, active: bool) -> TabInfo {
    TabInfo {
        position,
        name: format!("tab{}", position + 1),
        active,
        ..Default::default()
    }
}

pub(crate) fn status(pane_id: u32, event: &str) -> StatusPayload {
    StatusPayload {
        pane_id,
        event: event.to_string(),
        agent: "claude".to_string(),
        source: None,
        cwd: None,
        detail: None,
    }
}

// `SessionStart` に起動理由を添えたもの（配置演出のトリガー判定用）
pub(crate) fn session_start(pane_id: u32, source: &str) -> StatusPayload {
    StatusPayload {
        source: Some(source.to_string()),
        ..status(pane_id, "SessionStart")
    }
}

// 番号列だけを持つ先頭列（マーク列は出さないフレーム）
pub(crate) fn number_cells(number: Option<(&str, bool)>) -> HeadCells<'_> {
    HeadCells {
        number,
        ..HeadCells::default()
    }
}

// タブ0に count 個のターミナルペイン（ID 1..=count）を持つ状態
pub(crate) fn state_with_panes(count: u32) -> State {
    let panes: Vec<PaneInfo> = (1..=count)
        .map(|i| terminal_pane(i, &format!("pane{}", i)))
        .collect();
    let mut state = State {
        tabs: vec![tab(0, true)],
        panes: Some(manifest(vec![(0, panes)])),
        permissions_granted: true,
        ..Default::default()
    };
    state.rebuild_selectable();
    state
}

// --- レイアウトの定数と計測（決定202608060052・決定202608060053） ---

// 既定のサイドバー幅（決定202607302256）
pub(crate) const SIDEBAR: usize = 32;
// ツリーの上に常時居る枠（境界線・ヘッダー・境界線）。ツリーの行番号は
// すべてこの下から数える（要件: sidebar-header.feature）
pub(crate) const HEADER_ROWS: usize = 3;
// ツリーの下に常時居る枠（境界線・フッター・status-bar と離すための余白）
pub(crate) const FOOTER_ROWS: usize = 3;
// 右マージン2セルを除いた、文字を置ける幅
pub(crate) const CONTENT: usize = SIDEBAR - 2;

// 1ペインだけを持つ状態。ペイン名を指定して作る
pub(crate) fn state_with_one_pane(title: &str) -> State {
    let mut state = state_with_panes(0);
    state.panes = Some(manifest(vec![(0, vec![terminal_pane(1, title)])]));
    state.rebuild_selectable();
    state
}

// そのフレームのカウンタ列（描画と同じ手順で測る）
pub(crate) fn column_of(state: &State) -> CounterColumn {
    state.counter_column(&state.visible_rows())
}

// content 内で needle が始まる列（表示セル基準）
pub(crate) fn column_at(content: &str, needle: &str) -> usize {
    let byte = content
        .find(needle)
        .unwrap_or_else(|| panic!("{:?} が {:?} に無い", needle, content));
    unicode_width::UnicodeWidthStr::width(&content[..byte])
}

pub(crate) fn repeat_status(state: &mut State, pane_id: u32, event: &str, times: usize) {
    for _ in 0..times {
        state.apply_status(status(pane_id, event));
    }
}

// --- 装飾レベルの読み出し ---

// Text の装飾を「レベル → 文字位置」に戻す。serialize() は
// 「`selected`/`opaque` のプレフィックス → レベルごとの位置列を `$` 区切りで
// 並べたもの → 本文」の形。色は 0-3、dim は 4（zellij-tile の Text の取り決め）。
//
// **プレフィックスは zellij 本体と同じ順（x → z）で剥がす。** 本体は
// `parse_selected` → `parse_opaque` の順に先頭1文字ずつ見るので、剥がし残しは
// そのままレベル0の先頭の数値にくっついて位置指定を壊す。ここで同じ順を踏むことで、
// 実機と同じ見え方を検査できる（docs/issues/issue-idle-icon-color-on-selection.md）
pub(crate) fn ink_levels(text: &Text) -> Vec<Vec<usize>> {
    let mut serialized = text.serialize();
    for marker in ['x', 'z'] {
        if serialized.starts_with(marker) {
            serialized.remove(0);
        }
    }
    let Some((indices, _body)) = serialized.rsplit_once('$') else {
        return Vec::new();
    };
    indices
        .split('$')
        .map(|level| {
            level
                .split(',')
                .filter_map(|position| position.parse().ok())
                .collect()
        })
        .collect()
}

// そのレベルの装飾が乗っている文字位置（乗っていなければ空）
pub(crate) fn ink_at(text: &Text, level: usize) -> Vec<usize> {
    ink_levels(text).get(level).cloned().unwrap_or_default()
}

// opaque（選択行の背景の帯）が乗っているか。プレフィックスは ink_levels と
// 同じ並び（x → z）なので、selected の有無に関わらず判定できる
pub(crate) fn is_opaque(text: &Text) -> bool {
    let serialized = text.serialize();
    serialized
        .strip_prefix('x')
        .unwrap_or(&serialized)
        .starts_with('z')
}

pub(crate) const DIM_LEVEL: usize = 4;
// unbold。zellij 側の基底スタイルが bold なので、落とさない＝太いまま残る
pub(crate) const UNBOLD_LEVEL: usize = 5;
// error_color。状態アイコン `error` と終了操作サブモードの警告色（決定202608080140）
pub(crate) const ERROR_LEVEL: usize = 6;

// --- 設定・枠・オーバーレイ ---

// プラグインの configuration（決定202608080346。取り込みは State::apply_config 1本）
pub(crate) fn plugin_config(settings: &[(&str, &str)]) -> BTreeMap<String, String> {
    settings
        .iter()
        .map(|(setting, value)| (setting.to_string(), value.to_string()))
        .collect()
}

// READMEが例示している direct-keys の移動キー（Alt Up / Alt Down / Alt g）。
// `toggle_cwd_key` は入れない — 幅の詰め方（矢印への退避・末尾の省略）を見る
// テストが多く、移動キー3つで既に幅32を超えるため
pub(crate) fn with_direct_keys(state: &mut State) {
    state.apply_config(&plugin_config(&[
        ("up_key", "alt+up"),
        ("down_key", "alt+down"),
        ("go_key", "alt+g"),
    ]));
}

// 枠（境界線→ヘッダー→境界線→…→境界線→フッター）が崩れていないこと
pub(crate) fn assert_frame(rows: &[Row<'_>], label: &str) {
    assert!(
        matches!(
            (&rows[0], &rows[1], &rows[2]),
            (Row::Divider, Row::Header, Row::Divider)
        ),
        "{} で上の枠が崩れた",
        label
    );
    assert!(
        matches!(
            (
                &rows[rows.len() - 3],
                &rows[rows.len() - 2],
                &rows[rows.len() - 1]
            ),
            (Row::Divider, Row::Footer, Row::Blank)
        ),
        "{} で下の枠が崩れた",
        label
    );
}

// ヘルプオーバーレイに実際に載る行（モードごとのキー一覧＋共通の状態アイコン凡例）。
// 高さは全部載るだけ渡す — あふれ方の検証は別のテストで見る
pub(crate) fn overlay_lines(state: &State, cols: usize) -> Vec<String> {
    state
        .screen_rows(40)
        .iter()
        .filter_map(|row| match row {
            Row::Help(help) => Some(state.help_line(help, cols).content().to_string()),
            _ => None,
        })
        .collect()
}

pub(crate) fn overflow_markers(state: &State, rows: usize) -> Vec<(usize, bool)> {
    state
        .screen_rows(rows)
        .iter()
        .filter_map(|row| match row {
            Row::Overflow { hidden, above } => Some((*hidden, *above)),
            _ => None,
        })
        .collect()
}

// --- キー入力とサブモードへの入場 ---

pub(crate) fn key(bare: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare)
}

pub(crate) fn type_query(state: &mut State, query: &str) {
    for c in query.chars() {
        state.handle_nav_key(key(BareKey::Char(c)));
    }
}

pub(crate) fn jump_state(count: u32) -> State {
    let mut state = state_with_panes(count);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('n')));
    state
}

pub(crate) fn termination_state(count: u32) -> State {
    let mut state = state_with_panes(count);
    state.nav_mode = true;
    state.handle_nav_key(key(BareKey::Char('d')));
    state
}

// タブ2枚（tab1: alpha, bravo / tab2: charlie）。bravo だけ cwd を持つ
pub(crate) fn searchable_state() -> State {
    let mut state = State {
        tabs: vec![tab(0, true), tab(1, false)],
        panes: Some(manifest(vec![
            (
                0,
                vec![terminal_pane(1, "alpha"), terminal_pane(2, "bravo")],
            ),
            (1, vec![terminal_pane(3, "charlie")]),
        ])),
        permissions_granted: true,
        nav_mode: true,
        ..Default::default()
    };
    state.pane_cwds.insert(2, "/work/fujin".to_string());
    state.rebuild_selectable();
    state
}

// いまの検索サブモードの状態（決定202608131200）
pub(crate) fn search_phase(state: &State) -> Option<SearchPhase> {
    state.search.as_ref().map(|s| s.phase)
}

// 検索サブモードの操作状態まで進める（`/` で入って Esc）
pub(crate) fn navigating_search(query: &str) -> State {
    let mut state = searchable_state();
    state.handle_nav_key(key(BareKey::Char('/')));
    type_query(&mut state, query);
    state.handle_nav_key(key(BareKey::Esc));
    state
}

// タブ0に3ペイン、タブ1に1ペインを持つ navモード中の状態
pub(crate) fn triage_state() -> State {
    let mut state = State {
        tabs: vec![tab(0, true), tab(1, false)],
        panes: Some(manifest(vec![
            (
                0,
                vec![
                    terminal_pane(1, "alpha"),
                    terminal_pane(2, "bravo"),
                    terminal_pane(3, "charlie"),
                ],
            ),
            (1, vec![terminal_pane(4, "delta")]),
        ])),
        permissions_granted: true,
        nav_mode: true,
        ..Default::default()
    };
    state.rebuild_selectable();
    state
}

// フックのイベント列を通してエージェント状態を作る（直接代入せず、
// シーケンス番号も本番と同じ経路で振らせる）
pub(crate) fn set_agent_state(state: &mut State, pane_id: u32, target: AgentState) {
    match target {
        AgentState::Idle => {
            state.apply_status(status(pane_id, "SessionStart"));
        }
        AgentState::Working => {
            state.apply_status(status(pane_id, "UserPromptSubmit"));
        }
        AgentState::Blocked => {
            state.apply_status(status(pane_id, "Notification"));
        }
        AgentState::Done => {
            state.apply_status(status(pane_id, "UserPromptSubmit"));
            state.apply_status(status(pane_id, "Stop"));
        }
        AgentState::Error => {
            state.apply_status(status(pane_id, "StopFailure"));
        }
    }
}

// --- コマンドペインと既読 ---

pub(crate) fn command_pane(id: u32, command: &str) -> PaneInfo {
    PaneInfo {
        id,
        terminal_command: Some(command.to_string()),
        ..Default::default()
    }
}

// 終了して残っているコマンドペイン（`close_on_exit` は既定 false なので、
// プロセスが終わってもペインは `exited` のまま残り続ける）
pub(crate) fn exited_command_pane(id: u32, command: &str, exit_status: Option<i32>) -> PaneInfo {
    PaneInfo {
        exited: true,
        exit_status,
        ..command_pane(id, command)
    }
}

// 与えたペインを持つタブ0だけの状態。導出（apply_command_states）まで済ませる
pub(crate) fn state_with_command_panes(panes: Vec<PaneInfo>) -> State {
    let mut state = state_with_panes(0);
    let manifest = manifest(vec![(0, panes)]);
    state.apply_command_states(&manifest);
    state.panes = Some(manifest);
    state.rebuild_selectable();
    state
}

pub(crate) fn focused(pane: PaneInfo) -> PaneInfo {
    PaneInfo {
        is_focused: true,
        ..pane
    }
}

// --- pipe（ワイヤプロトコル） ---

pub(crate) fn pipe_message(name: &str, payload: &str) -> PipeMessage {
    PipeMessage {
        source: PipeSource::Plugin(0),
        name: name.to_string(),
        payload: Some(payload.to_string()),
        args: BTreeMap::new(),
        is_private: true,
    }
}

// --- 配置演出（要件: docs/requirements/req-header-animation.md） ---

// ペイン一覧を差し替えて1回ぶん観測させる。`Event::PaneUpdate` の扱いと同じ順序。
// **配置演出のトリガーはもう一覧を見ない**（フック通知だけで判定する）ので、ここでは
// ヘッダー描画に要る状態を作るだけ
pub(crate) fn observe_panes(state: &mut State, ids: &[u32]) {
    let panes: Vec<PaneInfo> = ids
        .iter()
        .map(|id| terminal_pane(*id, &format!("pane{}", id)))
        .collect();
    state.panes = Some(manifest(vec![(0, panes)]));
    state.rebuild_selectable();
}

// まだ何も観測していない、既定幅で描画済みのサイドバー
pub(crate) fn sidebar_state() -> State {
    State {
        tabs: vec![tab(0, true)],
        permissions_granted: true,
        viewport_cols: SIDEBAR,
        ..Default::default()
    }
}
