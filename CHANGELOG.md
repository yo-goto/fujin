## [0.1.0] - 2026-09-06

### 🚀 Features

- Split pane rows into three columns, giving cwd its own line
- Add triage mode (decision 23)
- Add vertical scrolling to the sidebar (decision 24)
- Fix the sidebar header at three lines
- Add a status icon legend to the help overlay (decision 25)
- Add the number jump sub-mode (decision 29)
- Add command status (decision 32)
- Drop the working pane's frame while in nav mode (decision 34)
- Borrow focus instead of dropping the frame while in nav mode (decision 34)
- Drop bold from pane names in the sidebar (decision 36)
- Add a deployment animation to the header (requirement: header-animation)
- Add a grace period before marking a pane read (decision 37)
- Add termination actions for the selected pane (decision 35)
- *(readme)* Swap the README header logo for the animated version
- *(setup)* Cut setup steps by generating the layout scaffold (decision 38)
- *(setup)* Add release distribution and troubleshooting
- Allow batch termination via multi-select marking (decision 39)
- *(config)* Unify the settings intake path and warn on unparsable values in the footer
- *(config)* Let show_deploy_animation disable the deployment animation
- *(sidebar)* Wrap floating pane names in parentheses
- *(config)* Let toggle_cwd_key toggle cwd display at runtime
- *(preview)* Preview the selected pane in a floating pane during nav mode
- *(extras)* Let another worktree's build be tried without touching the resident session
- *(extras)* Add --open to try-worktree.sh to launch in a separate window
- *(setup)* Add --resizable to write the sidebar width as a percentage
- Provide a dev environment via a Nix flake (Rust wasm32-wasip1 + git-cliff + zellij)
- *(deploy)* Trigger the deployment animation on receiving the SessionStart hook instead
- *(gitleaks)* Add a custom rule and replace absolute paths in current files
- *(sidebar)* Show a not-deployed marker on panes without an agent
- *(preview)* Dock the preview to the sidebar's right edge and shrink its width to 35%
- *(preview)* Add a symbol to the preview pane's title
- *(nav)* Swap preview to `p` and triage entry to `t`
- *(sidebar)* Keep the sidebar width in sync across tabs
- *(try-worktree)* Add a --theme option to preview a different color theme
- *(search)* Split the search sub-mode into editing and navigating states
- *(sidebar)* Add optional ~ shortening for the cwd row
- *(sidebar)* Group the shared modifier in the direct-keys hint
- *(kbd-harness)* Replay out.log into the screen it drew
- *(try)* Let a worktree keep its own fujin persistence store
- *(setup)* Name the command that installs jq on this machine

### 🐛 Bug Fixes

- Stop the counter from disappearing when the pane name runs long
- Leave a 2-cell margin at the sidebar's right edge
- Keep the header at a constant one line to remove the shift on nav mode entry/exit
- Align the overflow marker row to the same column as the tab heading
- Make the error state mean StopFailure only
- Fall back to cwd when the pane name is empty (decision 26)
- Push the footer to the bottom when the tree is short
- Make the notification row's text English
- *(sidebar)* Drop the cwd row back to normal weight when unselected
- *(render)* Fix truncation bleeding into rows without a counter badge
- *(sidebar)* Revert the cwd row's unbold change back to dim+bold
- *(sidebar)* Keep the cwd row dim even while selected
- *(sidebar)* Round the leading truncation of a cwd path to a `/` boundary
- *(preview)* Request the ReadPaneContents permission that get_pane_scrollback needs
- *(extras)* Fix try-worktree.sh's pre-registered permissions not tracking newly added ones
- *(sidebar)* Retract the cwd row too when the agent exits
- *(sidebar)* Fix the idle icon turning white on the selected row
- *(nav)* Fix non-ASCII input via IME turning into a different key
- *(sidebar)* Place the real cursor on the input field so the IME candidate window follows it
- *(sidebar)* Move the real cursor notification outside of render()
- *(nav)* Catch text lost on IME conversion confirmation via PastedText
- *(sidebar)* Hide the real cursor when no input field is showing
- *(sidebar)* Stop redrawing on external events while typing
- *(nav)* Stop paste during the help overlay from leaking into the query
- *(sidebar)* Never place the real cursor outside the pane even when the query overflows the field
- *(sidebar)* Keep the real cursor following even on InterceptedKeyPress outside nav mode
- *(entry)* Log host decode failures instead of silently dropping them
- *(sidebar)* Stop at the nearest reachable width when the target width doesn't land on a step
- *(try-worktree)* Prevent a silent skip via set -e at install_config's && chain
- *(search)* Unify the search state indicator on the real cursor
- *(try)* Make verification session cleanup reliable so it doesn't trigger a client panic
- *(sync)* Have a new tab's sidebar fetch state from its siblings
- *(features)* Remove the formations requirement not yet merged into main
- *(extras)* Prefer alacritty as try-worktree.sh's default --open terminal
- *(extras)* Fix try-worktree.sh silently swallowing --open failures and exceeding the session name limit
- *(nav)* Stop restoring a stale exploring position after an interrupted exit
- *(nav)* Exit nav mode when focus leaves the tab
- *(nav)* Anchor the exit focus to the jump target
- *(try)* Stop one agent's cleanup from destroying another's session
- *(render)* Draw dividers to the full width so they line up
- *(render)* Stop the selection highlight at the right margin instead
- *(setup)* Stop --download --dry-run from failing outside a clone

### 💼 Other

- Sidebar with agent status, hook bridge, and pipe navigation
- Add Makefile/test/lint policy to the development section
- Introduce git-cliff and add CHANGELOG generation
- *(nix)* Move devShell dependencies into packages, noting the version-drift caveat
- *(changelog)* Replace cliff.toml's placeholder with the real `https://github.com/yo-goto/fujin` URL
- Merge main (fix the no-reflow scope in 4a87657's feature)
- Add --locked to cargo invocations that resolve dependencies
- *(changelog)* Switch make changelog to --prepend so past entries survive

### 🚜 Refactor

- Core cleanup: compress comments, tidy modules, remove duplication
- *(search)* Tidy up the state indicator's terminology and hint text
- *(tests)* Hoist cross-cutting helpers into src/test_support.rs
- *(tests)* Inline tests for pure functions that don't touch State into width.rs / agent.rs
- *(tests)* Extract sidebar width sync across tabs into src/tests/sidebar_width.rs
- *(tests)* Extract row clicks into src/tests/click_to_focus.rs
- *(tests)* Extract ad-hoc summon into src/tests/summon.rs
- *(tests)* Extract the pipe wire protocol into src/tests/pipe_protocol.rs
- *(tests)* Extract state dump and sibling instance detection into src/tests/instance_sync.rs
- *(tests)* Extract the number jump sub-mode into src/tests/pane_number_jump.rs
- *(tests)* Extract the termination sub-mode into src/tests/pane_close_kill.rs
- *(tests)* Extract the floating pane indicator into src/tests/floating_pane_indicator.rs
- *(tests)* Extract preview into src/tests/preview.rs
- *(tests)* Extract non-ASCII input via IME into src/tests/ime_input.rs
- *(tests)* Extract agent status transitions, the read-receipt model, and the grace period into src/tests/agent_status.rs
- *(tests)* Extract nav mode key handling and the help overlay into src/tests/nav_mode.rs
- *(tests)* Extract selectable rows, the footer, and vertical scrolling into src/tests/sidebar_tree.rs
- *(tests)* Extract focus sync and focus borrowing into src/tests/focus_sync.rs
- *(tests)* Extract the three mark-related sections into src/tests/pane_termination_multi_select.rs
- *(tests)* Extract the search sub-mode into src/tests/search_explorer.rs
- *(tests)* Extract the three command status sections into src/tests/command_status.rs
- *(tests)* Extract triage mode into src/tests/triage_mode.rs
- *(tests)* Extract pane row layout, the cwd row, and pane name fallback into src/tests/pane_row.rs
- *(tests)* Extract the deployment animation into src/tests/header_animation.rs
- *(tests)* Extract settings intake and warnings into src/tests/configuration.rs, leaving tests.rs as a bare mod-declaration file
- *(nav)* Move the number jump and search sub-modes out of nav.rs into their own modules
- *(focus)* Gather focus sync and focus borrowing into focus.rs
- *(sync)* Split sidebar width sync out into width_sync.rs
- *(config)* Move settings intake and warning expiry management into config.rs
- *(render)* Split render.rs into a child module per frame element
- Align this branch's private-note references on the .docs root
- *(setup)* Check for curl and jq before writing anything

### 📚 Documentation

- Reflect the status icon legend in the Japanese README too
- Correct a comment assuming unselectable panes can't be closed
- Align the nav mode key list with decision 29's implementation
- *(readme)* Correct how to change the sidebar width to match measured behavior
- *(readme)* Align the width notation's name with the terminology table
- *(readme)* Spell out the inside-declaration as a design principle
- *(keybinds)* Fix the example comment's Alt Up/Alt Down notation to Alt u/Alt d
- Move feature requirements (.feature) into the same repository as the code
- Switch decision number identifiers to the 12-digit timestamp form to match the docs/decisions/ split
- Align extras/'s decision numbers to the same 12-digit identifier
- Fix a comment's requirement-note path to point at a file that actually exists
- Reflect the issue- prefix in a comment's investigation-note path
- Fix a hook comment's design-note path to match the current layout
- *(nav)* Align the module comment's responsibility description with the post-split reality
- Move the module structure reference to module-map.md
- Align private-note references on the .docs root
- *(config)* Point the log hint at a public command
- *(readme)* Fix nav-mode header/footer drift, add triage section
- *(readme)* Reconcile the usage sections with the implementation
- *(readme)* Go back to the icon-only logo and drop the tree sample
- *(readme)* Reorder the sections and cut the explanation down
- *(readme)* Correct what the compression pass got wrong
- *(readme)* Give the width tweak a command that works off a release
- *(changelog)* Regenerate CHANGELOG.md and translate pre-2026-08-24 entries
- *(readme)* Add a screenshot of the sidebar in use
- *(readme)* State jq as required rather than an aside

### 🧪 Testing

- Fill gaps found while auditing triage mode's requirements
- *(deploy)* Pin source parsing to the payload shape the hook actually emits
- Introduce a host command indirection layer (host.rs) so issuance can be verified in tests
- *(sidebar-tree)* Cover every mode in sidebar-header.feature's no-reflow scenario
- *(sidebar-tree)* Drop two out-of-scope modes from the row-position invariant scenario
- Confine serialize-format parsing to test_support, and spell out the standard for nav mode's two entry paths
- *(render)* Guard frame invariants with assertions, and split row assembly out into Line
- Wire up golden files via insta, and check cleanup with check-snapshots
- Add 6 measured boundary widths to the exhaustive-width test
- *(render)* Add 5 golden files, bringing the total to 6
- *(render)* Give the row-position invariant an executable backing too
- *(render)* Add a golden file for the tilde-shortened cwd row

### ⚙️ Miscellaneous Tasks

- *(assets)* Reflect logo animation tweaks in the distributed assets
- *(assets)* Reflect the landing-to-formation logo animation in the distributed assets
- *(gitleaks)* Retire .gitleaksignore now that history has been rewritten
- Rename requirements/ to features/
