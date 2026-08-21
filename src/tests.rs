// 単体テスト。
//
// 実行はホストターゲットで行う（`make test`）。既定ターゲットの wasm32-wasip1 では
// テストバイナリを走らせるランタイムがないため。
//
// このファイルは `src/tests/` 配下への配線だけを持つ。テストは機能・要件単位で
// 1ファイルずつに分かれていて（ファイル名は対応する docs/requirements/ のスラッグ）、
// 複数のファイルから使うヘルパ・定数と `host_run_plugin_command` のリンクスタブ、
// テストできる範囲（呼べないホスト関数）の説明は `src/test_support.rs` にある。
// State に触れない純粋関数のテストは、search.rs と同じく各実装ファイルの
// `#[cfg(test)] mod tests` にインラインで置く（width.rs・agent.rs）。

mod agent_status;
mod click_to_focus;
mod command_status;
mod configuration;
mod floating_pane_indicator;
mod focus_sync;
mod frame_invariants;
mod header_animation;
mod ime_input;
mod instance_sync;
mod nav_mode;
mod pane_close_kill;
mod pane_number_jump;
mod pane_row;
mod pane_termination_multi_select;
mod pipe_protocol;
mod preview;
mod search_explorer;
mod sidebar_tree;
mod sidebar_width;
mod summon;
mod triage_mode;
