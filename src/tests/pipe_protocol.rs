use crate::agent::AgentState;
use crate::test_support::*;
use crate::*;

// --- pipe（ワイヤプロトコル） ---

// キーバインドからの `fujin_toggle_cwd` はユーザー操作なので payload を持たない
//（NAV_UP_PIPE等と同じ）。同期用の明示セットとの分岐を試すのに必要
fn pipe_message_no_payload(name: &str) -> PipeMessage {
    PipeMessage {
        payload: None,
        ..pipe_message(name, "")
    }
}

#[test]
fn status_pipe_applies_the_payload() {
    let mut state = State::default();
    let rendered = state.pipe(pipe_message(
        STATUS_PIPE,
        r#"{"pane_id":"4","event":"UserPromptSubmit","agent":"claude"}"#,
    ));
    assert!(rendered);
    assert_eq!(state.agents[&4].state, AgentState::Working);
}

#[test]
fn status_pipe_ignores_garbage() {
    let mut state = State::default();
    assert!(!state.pipe(pipe_message(STATUS_PIPE, "{}")));
    assert!(state.agents.is_empty());
}

#[test]
fn selection_pipe_is_addressed_by_pane_id() {
    // インデックスではなくペインIDで運ぶので、一覧が違っても同じ行を指す
    let mut state = state_with_panes(3);
    assert!(state.pipe(pipe_message(SELECTION_PIPE, "3")));
    assert_eq!(state.selected, 2);
    // 同じ位置への再指定は再描画不要
    assert!(!state.pipe(pipe_message(SELECTION_PIPE, "3")));
    // 知らないペインIDは無視する
    assert!(!state.pipe(pipe_message(SELECTION_PIPE, "99")));
    assert_eq!(state.selected, 2);
}

#[test]
fn read_clear_pipe_clears_listed_panes() {
    let mut state = state_with_panes(3);
    state.apply_status(status(1, "Stop"));
    state.apply_status(status(2, "Stop"));
    state.apply_status(status(3, "UserPromptSubmit"));

    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1,3")));
    assert_eq!(state.agents[&1].state, AgentState::Idle);
    assert_eq!(state.agents[&2].state, AgentState::Done);
    // working は既読対象ではないので触らない
    assert_eq!(state.agents[&3].state, AgentState::Working);
}

#[test]
fn sync_pipe_fills_unknown_panes_without_overwriting_known_ones() {
    let dump = "1\tdone\t0\t0\tclaude\t\n2\tdone\t0\t0\tclaude\t\n";

    let mut empty = state_with_panes(2);
    assert!(empty.pipe(pipe_message(SYNC_STATE_PIPE, dump)));
    assert_eq!(empty.agents[&1].state, AgentState::Done);
    assert_eq!(empty.agents[&2].state, AgentState::Done);

    // 自前の登録があるペインは上書きしない。フック通知は全インスタンスへ届くので、
    // 登録さえあれば中身は自分のほうが確か
    let mut populated = state_with_panes(2);
    populated.apply_status(status(1, "UserPromptSubmit"));
    assert!(populated.pipe(pipe_message(SYNC_STATE_PIPE, dump)));
    assert_eq!(populated.agents[&1].state, AgentState::Working);
    // 知らなかったペインは埋まる（以前はダンプを丸ごと捨てていて埋まらなかった）
    assert_eq!(populated.agents[&2].state, AgentState::Done);
}

#[test]
fn sync_pipe_request_does_not_touch_the_local_state() {
    // `?` は「状態を教えてほしい」という新入りからの問い合わせ。ダンプとして
    // 解釈させない（送り返す先は pipe_message_to_plugin なので、ここでは
    // 取り込まないことだけを見る）
    let mut state = state_with_panes(1);
    state.apply_status(status(1, "UserPromptSubmit"));

    assert!(!state.pipe(pipe_message(SYNC_STATE_PIPE, "?")));
    assert_eq!(state.agents.len(), 1);
    assert_eq!(state.agents[&1].state, AgentState::Working);
}

#[test]
fn toggle_cwd_pipe_flips_the_local_value_when_unprompted() {
    // ユーザーのキー操作からはpayloadが付かない（決定202607302258）。全インスタンスが
    // 同じ値から出発している前提で、権威を立てず各自が独立に反転する
    // （docs/issues/toggle-cwd-key.md）
    let mut state = State::default();
    assert!(!state.show_cwd);

    assert!(state.pipe(pipe_message_no_payload(TOGGLE_CWD_PIPE)));
    assert!(state.show_cwd);

    assert!(state.pipe(pipe_message_no_payload(TOGGLE_CWD_PIPE)));
    assert!(!state.show_cwd);
}

#[test]
fn toggle_cwd_pipe_applies_an_explicit_sync_value() {
    // 新入りインスタンスへの現在値push（決定202608012141）は明示セット。反転にすると
    // 押し付けるたびに向きがずれる
    let mut state = State::default();
    assert!(state.pipe(pipe_message(TOGGLE_CWD_PIPE, "true")));
    assert!(state.show_cwd);

    // 既に同じ値なら再描画不要
    assert!(!state.pipe(pipe_message(TOGGLE_CWD_PIPE, "true")));

    assert!(state.pipe(pipe_message(TOGGLE_CWD_PIPE, "false")));
    assert!(!state.show_cwd);
}

#[test]
fn toggle_cwd_pipe_treats_an_empty_payload_as_a_key_press() {
    // CLI から `zellij pipe` で叩くと payload が空文字で届きうる。config.rs の
    // 正規化と同じく未設定扱いにして、キー操作（反転）として読む
    let mut state = State::default();
    for payload in ["", "  ", "\n"] {
        let before = state.show_cwd;
        assert!(
            state.pipe(pipe_message(TOGGLE_CWD_PIPE, payload)),
            "{payload:?}"
        );
        assert_eq!(state.show_cwd, !before, "{payload:?}");
    }
}

#[test]
fn toggle_cwd_pipe_ignores_an_unparsable_payload() {
    // 真偽値の受け口は広げない（決定202608080346）。黙って false へ倒すと、cwd が消えた
    // 結果だけが残って原因を追えない
    let mut state = State {
        show_cwd: true,
        ..Default::default()
    };

    for payload in ["1", "yes", "TRUE", "false ish"] {
        assert!(
            !state.pipe(pipe_message(TOGGLE_CWD_PIPE, payload)),
            "{payload:?}"
        );
        assert!(state.show_cwd, "{payload:?}");
    }
}

#[test]
fn unknown_pipes_are_ignored() {
    let mut state = State::default();
    assert!(!state.pipe(pipe_message("some_other_plugin", "payload")));
}
