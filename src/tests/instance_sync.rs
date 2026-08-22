// インスタンス間同期（決定202608012141）: 状態ダンプのワイヤ形式と兄弟インスタンスの検出。
// マーク・コマンド状態それぞれの同期は各機能のファイル側にある

use crate::agent::AgentState;
use crate::test_support::*;
use crate::*;

// --- 状態ダンプ（インスタンス間同期のワイヤ形式） ---

#[test]
fn state_dump_round_trips() {
    let mut source = State::default();
    source.apply_status(status(1, "UserPromptSubmit"));
    source.apply_status(status(1, "SubagentStart"));
    // バックグラウンドのサブエージェントを残したままターンが終わった状態
    source.apply_status(status(1, "Stop"));
    source.apply_status(status(2, "TaskCreated"));
    source.pane_cwds.insert(1, "/work/fujin".to_string());

    let mut restored = State::default();
    restored.apply_state_dump(&source.state_dump());

    assert_eq!(restored.agents.len(), 2);
    assert_eq!(restored.agents[&1].state, AgentState::Working);
    assert_eq!(restored.agents[&1].subagents, 1);
    assert_eq!(restored.agents[&1].agent, "claude");
    // ターン終了済みも運ぶ。運ばないと同期先が最後の SubagentStop で done にできない
    assert!(restored.agents[&1].turn_ended);
    // シーケンス番号も運ぶ。運ばないと同期先のトリアージ一覧で同一階層内の
    // 並びが総崩れになる（全員0でツリー順に潰れる）
    assert_eq!(
        restored.agents[&1].state_change_seq,
        source.agents[&1].state_change_seq
    );
    assert!(
        restored.state_seq >= source.agents[&1].state_change_seq,
        "受け手のカウンタは配られた最大値まで進める"
    );
    restored.apply_status(status(1, "SubagentStop"));
    assert_eq!(restored.agents[&1].state, AgentState::Done);
    assert_eq!(
        restored.pane_cwds.get(&1).map(String::as_str),
        Some("/work/fujin")
    );
    assert_eq!(restored.agents[&2].open_tasks, 1);
    // cwd を持たないペインは登録しない
    assert!(!restored.pane_cwds.contains_key(&2));
}

#[test]
fn state_dump_skips_malformed_lines() {
    let mut state = State::default();
    state.apply_state_dump("garbage\nxx\tworking\t0\t0\tclaude\t\n3\tdone\t0\t0\tclaude\t\n");
    assert_eq!(state.agents.len(), 1);
    assert_eq!(state.agents[&3].state, AgentState::Done);
}

#[test]
fn state_dump_drops_panes_that_no_longer_exist() {
    // 送り手が閉じたばかりのペインを載せていても、受け手の一覧で間引く
    let mut state = state_with_panes(2);
    // 一覧が凍っていない（可視の）インスタンスの一覧だけを間引きに使う
    state.visible = true;
    state.apply_state_dump("1\tdone\t0\t0\tclaude\t\n99\tdone\t0\t0\tclaude\t\n");
    assert!(state.agents.contains_key(&1));
    assert!(!state.agents.contains_key(&99));
}

#[test]
fn state_dump_is_not_pruned_by_a_frozen_manifest() {
    // 非可視インスタンスの `PaneManifest` は凍っている。そこで間引くと、自分が
    // 非可視になったあとに生まれたペインの状態を受け取った端から捨ててしまう
    //（.docs/issues/issue-tab-switch-agent-status-desync.md）
    let mut state = state_with_panes(1);
    assert!(!state.visible);
    state.apply_state_dump("1\tdone\t0\t0\tclaude\t\n99\tdone\t0\t0\tclaude\t\n");
    assert!(state.agents.contains_key(&99));
}

#[test]
fn state_dump_fills_only_the_panes_it_does_not_know() {
    // 自前の登録は残し、欠けているペインだけを埋める。全か無かにすると、
    // 1件でも自前の登録があるインスタンスは欠けたまま直らない
    let mut state = state_with_panes(2);
    state.apply_status(status(1, "UserPromptSubmit"));

    assert!(state.apply_state_dump("1\tdone\t0\t0\tclaude\t\n2\tidle\t0\t0\tclaude\t\n"));
    assert_eq!(state.agents[&1].state, AgentState::Working);
    assert_eq!(state.agents[&2].state, AgentState::Idle);

    // 埋めるものが無ければ再描画も要らない
    assert!(!state.apply_state_dump("1\tdone\t0\t0\tclaude\t\n"));
}

// --- 兄弟インスタンスの検出（決定202608012141） ---

#[test]
fn own_plugin_url_is_learned_from_the_manifest() {
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        panes: Some(manifest(vec![(0, vec![plugin_pane(5, url)])])),
        ..Default::default()
    };
    state.learn_own_plugin_url();
    assert_eq!(state.own_plugin_url.as_deref(), Some(url));
}

#[test]
fn siblings_are_the_same_plugin_in_other_tabs() {
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        own_plugin_url: Some(url.to_string()),
        panes: Some(manifest(vec![
            (0, vec![plugin_pane(5, url), terminal_pane(1, "shell")]),
            (1, vec![plugin_pane(6, url)]),
            // 別プラグイン（status-bar 等）は兄弟ではない
            (2, vec![plugin_pane(7, "zellij:status-bar")]),
        ])),
        ..Default::default()
    };
    state.push_state_to_new_siblings();
    assert_eq!(
        state.known_siblings.iter().copied().collect::<Vec<_>>(),
        vec![6]
    );
}

#[test]
fn a_newcomer_asks_its_siblings_for_the_state_once() {
    // 新しいタブができた瞬間は新入りが可視・既存が全員非可視なので、押し付けの
    // 起点が既存側で発火しない。取りに行くのは新入り自身
    //（.docs/issues/issue-tab-switch-agent-status-desync.md）
    let url = "file:/x/fujin.wasm";
    let mut state = State {
        own_plugin_id: Some(5),
        own_plugin_url: Some(url.to_string()),
        panes: Some(manifest(vec![
            (0, vec![plugin_pane(6, url)]),
            (1, vec![plugin_pane(5, url), terminal_pane(1, "shell")]),
        ])),
        ..Default::default()
    };
    // 自分が生まれた後のフック通知で1件だけ埋まっていても要求する。空かどうかで
    // 分岐すると、それ以前から座っているペインの状態が欠けたまま直らない
    state.apply_status(status(1, "UserPromptSubmit"));

    state.push_state_to_new_siblings();
    assert!(state.state_requested);

    // 兄弟が増えても要求は繰り返さない。以降の変化はフック通知が全員に届く
    state.state_requested = false;
    state.push_state_to_new_siblings();
    assert!(
        !state.state_requested,
        "新しい兄弟が現れていないので要求もしない"
    );
}
