// コマンド状態（要件: .docs/requirements/req-command-status.md）: PaneManifest からの導出・
// インスタンス間同期・フォーカス中に終了したときの既読の猶予

use crate::agent::AgentState;
use crate::command::CommandState;
use crate::command::PaneStatus;
use crate::render::HeadCells;
use crate::test_support::*;
use crate::*;

// --- コマンド状態（決定202608072218。要件: .docs/requirements/req-command-status.md） ---
//
// コマンドペインの走行・終了を PaneManifest から導出する。エージェント状態とは
// 別概念だが、記号・既読モデル・待ち件数・トリアージ一覧は共用する

#[test]
fn a_running_command_pane_is_working() {
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn a_command_pane_that_exits_cleanly_is_done() {
    let state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn any_other_exit_is_an_error() {
    // 非0コードもシグナル終了（exit_status なし）も区別せず error（決定202608072218）
    let failed = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    assert_eq!(
        failed.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
    let signalled = state_with_command_panes(vec![exited_command_pane(1, "make", None)]);
    assert_eq!(
        signalled.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn a_shell_pane_has_no_command_state() {
    // コマンドペインでなければ状態を持たない（フックが無ければ状態を持たない
    // エージェント状態と対称）
    let state = state_with_command_panes(vec![terminal_pane(1, "zsh")]);
    assert_eq!(state.pane_status(1), None);
    assert!(state.commands.is_empty());
}

#[test]
fn trivial_commands_are_tracked_too() {
    // 絞り込みは行わない（決定202608072218）。実行時間の閾値のようなヒューリスティックは持たない
    let state = state_with_command_panes(vec![exited_command_pane(1, "ls", Some(0))]);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Done))
    );
}

#[test]
fn re_running_a_command_returns_it_to_working() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(1))]);
    let seq_before = state.status_seq(1);

    state.apply_command_states(&manifest(vec![(0, vec![command_pane(1, "make")])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert!(
        state.status_seq(1) > seq_before,
        "状態が変わったのでシーケンス番号も進む"
    );
}

#[test]
fn focusing_a_finished_command_pane_marks_it_read() {
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);

    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    assert_eq!(state.pane_status(1), None, "既読は状態を持たない側へ戻る");
}

#[test]
fn a_read_command_state_does_not_come_back() {
    // 終了したコマンドペインは exited が立ちっぱなしなので、既読を旗で持たないと
    // 次の PaneUpdate で同じ done が再導出されて復活する
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);

    state.apply_command_states(&manifest(vec![(
        0,
        vec![exited_command_pane(1, "make", Some(0))],
    )]));

    assert_eq!(state.pane_status(1), None);
}

#[test]
fn a_running_command_is_not_marked_read() {
    // working は既読にならない（走っている最中のコマンドは人を待っていない）
    let mut state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let focused = PaneInfo {
        is_focused: true,
        ..command_pane(1, "docker build .")
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Working))
    );
}

#[test]
fn the_agent_state_wins_over_the_command_state() {
    // `zellij run -- claude` のようにコマンドペイン経由でエージェントを起動した
    // ケース。フック由来の状態が常に優先で、コマンド状態は無視する（決定202608072218）
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "claude", Some(1))]);
    state.apply_status(status(1, "UserPromptSubmit"));

    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Agent(AgentState::Working))
    );
}

#[test]
fn command_states_join_the_waiting_count() {
    let state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        exited_command_pane(2, "cargo test", Some(101)),
        command_pane(3, "docker build ."),
        terminal_pane(4, "zsh"),
    ]);
    // done と error だけを数える（working は人の対応を待っていない）
    assert_eq!(state.waiting_count(), 2);
}

#[test]
fn command_states_join_the_triage_list() {
    let mut state = state_with_command_panes(vec![
        command_pane(1, "docker build ."),
        exited_command_pane(2, "cargo test", Some(101)),
        terminal_pane(3, "zsh"),
    ]);
    state.apply_status(status(3, "Notification"));

    let ids: Vec<u32> = state.triage_entries().iter().map(|e| e.pane_id).collect();
    // 優先度階層はソースを問わず共通（error → blocked → working → done）
    assert_eq!(ids, vec![2, 3, 1]);
}

#[test]
fn a_command_pane_without_a_name_shows_its_command() {
    // 決定202608072218: ペイン名が空ならコマンド文字列を代わりに出す
    let state = state_with_command_panes(vec![command_pane(1, "docker build .")]);
    let text = state.pane_row(
        &state.selectable[0],
        false,
        None,
        column_of(&state),
        HeadCells::default(),
        SIDEBAR,
    );
    let content = text.content();
    assert!(content.contains("docker build ."), "{}", content);
    assert!(content.contains('»'), "走行中のアイコンも出る: {}", content);
}

#[test]
fn a_named_command_pane_keeps_its_name() {
    // `zellij run --name` やリネームで名前が付いていればそちらが優先
    let named = PaneInfo {
        title: "build".to_string(),
        ..command_pane(1, "docker build .")
    };
    let state = state_with_command_panes(vec![named]);
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains("build"), "{}", content);
    assert!(!content.contains("docker"), "{}", content);
}

#[test]
fn closed_command_panes_lose_their_state() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    state.panes = Some(manifest(vec![(0, vec![command_pane(2, "docker build .")])]));
    state.prune_stale_agents();

    assert!(!state.commands.contains_key(&1));
    assert!(state.commands.contains_key(&2));
}

// --- コマンド状態のインスタンス間同期（決定202608012141・決定202608072218） ---

#[test]
fn the_command_dump_round_trips() {
    let mut state = state_with_command_panes(vec![
        exited_command_pane(1, "make", Some(0)),
        command_pane(2, "docker build ."),
    ]);
    // 1つを既読にしてから配る
    let focused = PaneInfo {
        is_focused: true,
        ..exited_command_pane(1, "make", Some(0))
    };
    state.apply_read_model(&manifest(vec![(0, vec![focused])]));
    settle_read(&mut state);
    let dump = state.command_dump();

    let mut peer = State::default();
    assert!(peer.apply_command_dump(&dump));

    assert_eq!(peer.pane_status(1), None, "既読も一緒に配る");
    assert_eq!(
        peer.pane_status(2),
        Some(PaneStatus::Command(CommandState::Working))
    );
    assert_eq!(peer.status_seq(2), state.status_seq(2));
    // 取り込んだ番号より古い番号を後から振らないよう、カウンタを進めておく
    assert!(peer.state_seq >= state.status_seq(2));
}

#[test]
fn a_broken_command_dump_line_is_skipped() {
    let mut state = State::default();
    assert!(!state.apply_command_dump("garbage\nx\tdone\t0\t1\n"));
    assert!(state.commands.is_empty());
}

#[test]
fn the_read_clear_pipe_also_clears_command_states() {
    // 既読クリアの配布（決定202608012141）はソースを区別しない
    let mut state = state_with_command_panes(vec![exited_command_pane(1, "make", Some(0))]);
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_command_state_pipe_takes_a_dump() {
    let mut state = State::default();
    assert!(state.pipe(pipe_message(COMMAND_STATE_PIPE, "7\terror\t0\t3\n")));
    assert_eq!(
        state.pane_status(7),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

// --- 既読の猶予（.docs/issues/issue-command-status-error-icon-swallowed.md） ---
//
// `zellij run` は新しいペインへフォーカスを移すので、一瞬で終わるコマンドは
// 必ず「フォーカス中に終了」する。素直に既読モデルを当てると、状態が付いた
// 同じ PaneUpdate の中で既読になり、アイコンが一度も描かれないまま消える

// 実セッションの再現手順（`zellij run -- sh -c 'exit 1'`）をそのままなぞる。
// 新しいコマンドペインがフォーカスを持ったまま走り、そのまま失敗して終わる
fn run_and_fail_while_focused() -> State {
    let mut state = state_with_panes(0);
    // 1回目の PaneUpdate: ペインが出来てフォーカスを持ち、まだ走っている
    let running = manifest(vec![(0, vec![focused(command_pane(1, "sh -c exit 1"))])]);
    state.apply_command_states(&running);
    state.apply_read_model(&running);
    state.panes = Some(running);
    state.rebuild_selectable();
    // 2回目の PaneUpdate: フォーカスを持ったまま失敗して終わる
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    state.apply_command_states(&exited);
    state.apply_read_model(&exited);
    state.panes = Some(exited);
    state.rebuild_selectable();
    state
}

#[test]
fn a_command_that_fails_while_focused_still_shows_its_icon() {
    let state = run_and_fail_while_focused();
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "状態が付いた瞬間にフォーカスしていても、その場では既読にしない"
    );
    let content = state
        .pane_row(
            &state.selectable[0],
            false,
            None,
            column_of(&state),
            HeadCells::default(),
            SIDEBAR,
        )
        .content()
        .to_string();
    assert!(content.contains('×'), "{}", content);
}

#[test]
fn the_read_grace_survives_repeated_updates() {
    // 描画のたびに PaneUpdate が飛ぶ（実測で数msおきに連続）。猶予が1回きり
    // だと、この連打の中で結局既読になってしまう
    let mut state = run_and_fail_while_focused();
    let exited = manifest(vec![(
        0,
        vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
    )]);
    for _ in 0..5 {
        state.apply_command_states(&exited);
        state.apply_read_model(&exited);
    }
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error))
    );
}

#[test]
fn leaving_the_pane_keeps_the_icon_and_coming_back_clears_it() {
    let mut state = run_and_fail_while_focused();
    // ユーザーが別のペインへ移る: 猶予は解けるが、既読にはならない
    let away = manifest(vec![(
        0,
        vec![
            exited_command_pane(1, "sh -c exit 1", Some(1)),
            focused(terminal_pane(2, "zsh")),
        ],
    )]);
    state.apply_command_states(&away);
    state.apply_read_model(&away);
    assert_eq!(
        state.pane_status(1),
        Some(PaneStatus::Command(CommandState::Error)),
        "離れただけでは既読にしない"
    );

    // 戻ってきて初めて既読になる
    let back = manifest(vec![(
        0,
        vec![
            focused(exited_command_pane(1, "sh -c exit 1", Some(1))),
            terminal_pane(2, "zsh"),
        ],
    )]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn another_tab_counts_as_having_left_the_pane() {
    // 別タブに居るあいだはフォーカスがそのペインに無いので猶予は解ける。
    // タブを切り替えて戻ってきたときに既読にならない、という取りこぼしを防ぐ
    let mut state = run_and_fail_while_focused();
    state.tabs = vec![tab(0, false), tab(1, true)];
    let elsewhere = manifest(vec![
        (0, vec![exited_command_pane(1, "sh -c exit 1", Some(1))]),
        (1, vec![focused(terminal_pane(2, "zsh"))]),
    ]);
    state.apply_command_states(&elsewhere);
    state.apply_read_model(&elsewhere);

    state.tabs = vec![tab(0, true), tab(1, false)];
    let back = manifest(vec![
        (
            0,
            vec![focused(exited_command_pane(1, "sh -c exit 1", Some(1)))],
        ),
        (1, vec![terminal_pane(2, "zsh")]),
    ]);
    state.apply_command_states(&back);
    state.apply_read_model(&back);
    settle_read(&mut state);
    assert_eq!(state.pane_status(1), None);
}

#[test]
fn the_read_clear_pipe_ignores_the_grace() {
    // 既読クリアの配布（決定202608012141）は、送り手が猶予込みで判断した結果。
    // 受け手が猶予で握り潰すと、タブごとにアイコンの有無が食い違う
    let mut state = run_and_fail_while_focused();
    assert!(state.pipe(pipe_message(READ_CLEAR_PIPE, "1")));
    assert_eq!(state.pane_status(1), None);
}
