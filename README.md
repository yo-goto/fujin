<div align="center">
  <img src="assets/logo.svg" alt="fujin logo" width="180" />
</div>

# fujin

A sidebar plugin for zellij. It lists tabs > panes in a vertical tree, visualizes
the state of AI agents (Claude Code, etc.) running in each pane, and lets you
jump to them with global keybindings.

- **Overview** — every tab and pane in the session, always visible in the sidebar
- **State at a glance** — an icon tells you whether each agent is working,
  waiting on you, or finished
- **Jump** — reach any pane with a couple of keystrokes or a single click, while
  your focus stays in your working pane (with fuzzy search over pane name, tab
  name, and cwd)

> [!NOTE]
> The name comes from the Japanese word 布陣 (*fujin*), "to deploy troops" / "to
> arrange a formation" — treating your panes as a formation to oversee at a
> glance. To English speakers, `fujin` also reads as 風神 (*fūjin*), the Japanese
> god of wind, pairing the stillness of forming up with the motion of wind
> sweeping across your panes.

## Design principles

> Local-first. Free. Zellij only. Just simple.

No network or cloud dependency — configuration lives entirely in local files.
Free forever, with no plan to bolt on cloud features to monetize later. Not a
general-purpose multiplexer replacement, but a zellij-only plugin that doesn't
take on features that would add complexity.

## Install

You need zellij 0.44 or later and `curl` (plus `jq` for the Claude Code hook).
**No Rust required.**

```bash
curl -fsSLO https://github.com/yo-goto/fujin/releases/latest/download/setup.sh
bash setup.sh --download
```

`setup.sh` puts the wasm and the hook script in `~/.config/zellij/plugins/`,
generates `~/.config/zellij/layouts/fujin.kdl`, and registers the hook for all 10
events in `~/.claude/settings.json`. **Updating is the same command** — it is
idempotent, backs up `settings.json`, and only asks before overwriting the layout.

It never edits `config.kdl`; it prints what to paste instead, since merging into
existing blocks is hard to recover from if text processing breaks it:

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}

// if you already have a keybinds block, add just the bind inside it
keybinds {
    shared_except "locked" {
        bind "Ctrl y" {
            MessagePlugin "fujin" {
                name "fujin_mode"
                floating true
            }
        }
    }
}

default_layout "fujin"
```

Restart zellij and the sidebar appears on the left. The first load asks for
permissions: focus that pane and press `y`. Approvals are recorded **per absolute
wasm path**, so moving the file means approving again.

### Sidebar width

The default `pane size=32` is a column count, which zellij's own resize doesn't
touch. Rewrite it as a **percentage** to make the sidebar resizable:

```sh
make setup SETUP_ARGS="--width 20% --layout-only"
```

The reliable way to resize is `Ctrl+n` for resize mode, then `h` (narrower) /
`H` (wider), `Esc` to leave. `Alt+-` works too, but `Alt+=` / `Alt++` need
`Shift` on many layouts and may never reach zellij.

Changing the width **pulls the sidebars in other tabs along** (with a percentage
only) — though dragging the border to a width off zellij's step grid (5% of the
terminal) only gets them to the nearest reachable one.

The trade-off is that the width now scales with the terminal, so a narrow terminal
gets a narrow sidebar (the on-screen text is laid out for 32 columns). A rewritten
layout only applies to new sessions.

<details>
<summary><code>setup.sh</code> options</summary>

| Option | What it does |
|---|---|
| `--download` | fetch the wasm and hook script from a release |
| `--version TAG` | which release to fetch (default `latest`) |
| `--dry-run` | write nothing; print what would be written |
| `--no-hooks` | skip registering the Claude Code hooks |
| `--layout-only` / `--hooks-only` / `--config-only` | run just that part |
| `--width N` | sidebar width (default 32). Written as `20%` it is treated as `--resizable` |
| `--resizable` | convert `--width` (columns) to a percentage of the terminal. Run inside a zellij pane it measures that pane, not the terminal, so passing a percentage directly is safer |
| `--force` | overwrite an existing layout file without asking |

</details>

<details>
<summary>Building from source</summary>

Needs Rust and `rustup target add wasm32-wasip1`.

```bash
git clone https://github.com/yo-goto/fujin.git && cd fujin
make setup   # build, install, generate the layout, register the Claude Code hooks
```

`make setup` includes `make install` (the release build and the copy into
`~/.config/zellij/plugins/`). Use `make install` on its own to swap just the wasm;
either way `PLUGIN_DIR=/path/to/plugins` changes the destination. Arguments for
`setup.sh` go through `SETUP_ARGS`, e.g. `make setup SETUP_ARGS="--no-hooks"`.
`make check` is the full verification.

</details>

## Keybindings

There are two routes — nav mode and direct keys — and they combine. Both go in
the keybinds block of `config.kdl` (`shared_except "locked"`, say).

### Nav mode (recommended)

It feels like zellij's own `Ctrl+p` pane mode. You bind **one key to enter** (the
snippet in the install section above) and the plugin interprets the keys inside the
mode itself, so no built-in mode is sacrificed and new keys never need a
`config.kdl` change. Change `Ctrl+y` if something else has taken it
(`p`/`t`/`n`/`h`/`s`/`o`/`q`/`g` are taken in zellij's defaults).

> [!IMPORTANT]
> Always include `floating true`. When no fujin is running in the session this
> pipe has no recipient, so zellij launches one — tiled by default, which splits
> your working pane and appears on the right, and the plugin cannot undo that.

### Direct keys (no mode)

For single-keystroke actions:

```kdl
bind "Alt u" {
    MessagePlugin "fujin" { name "fujin_up"; }
}
bind "Alt d" {
    MessagePlugin "fujin" { name "fujin_down"; }
}
bind "Alt g" {
    MessagePlugin "fujin" { name "fujin_go"; }
}
bind "Alt c" {
    MessagePlugin "fujin" { name "fujin_toggle_cwd"; }
}
```

`fujin_toggle_cwd` isn't a move but a display toggle: it flips the cwd row
(`show_cwd`) at runtime. Every tab switches together, and tabs created afterwards
inherit the current state, which makes the `show_cwd` setting the initial value.

> [!CAUTION]
> Don't bind `Alt Enter`. Claude Code's Shift+Enter relies on a terminal setting
> (`Shift+Return -> ESC CR`, installed by `/terminal-setup`), and zellij reads
> that `ESC CR` as `Alt Enter`. Taking it stops Shift+Enter from reaching the pane.

Don't use `MessagePluginId` either. The sidebar runs one instance per tab, so an
ID picks a single one and the keys collide; `MessagePlugin` with the alias reaches
every instance.

## Usage

Everything works while your focus stays in your working pane — you never need to
focus the sidebar itself (in fact it's excluded from focus cycling entirely).

### Reading the sidebar

Every pane row starts with a one-character state icon, and each state has its own
color. **The legend lives in the plugin**: press `?` in nav mode and it sits right
under the key list, so you never have to come back here to read it. Panes without
an agent (a plain shell, say) show `›` in that same spot, uncolored.

- The counters after a pane name are `+N` for active subagents and `[N]` for
  incomplete tasks
- A floating pane has its name wrapped in parentheses, `(name)`
- A pane with no name shows its command line instead, or its cwd if it isn't a
  command pane
- When the list is taller than the screen it scrolls to keep the selected row
  visible, and hidden rows are announced by `▴ … N more` / `▾ … N more`

**Command panes get the same icons** with no hook or setup — anything started as
one (`zellij run -- docker build .`, a `command` block in a layout, …) is
`working` while it runs, `done` on exit code 0, and `error` otherwise (non-zero,
signal, or interrupt). They never carry `blocked` or `idle`, and if an agent is
registered on the same pane, the agent's state wins.

`done` / `blocked` / `error` are **cleared automatically once you focus that pane**
(a read-receipt model). Merely passing through doesn't count: the focus has to
stay put for a moment before a state is marked read.

### Click to jump

Left-clicking a pane row jumps to that pane, with no mode to enter. A pane in
another tab brings that tab along; a floating pane brings up the floating layer.
Clicking a tab heading, or the empty space below the list, does nothing. If
nothing happens, check that zellij's `mouse_mode` is still enabled (it is by
default).

### Nav mode

`Ctrl+y` changes the header to `▲ fujin  [nav]` and the footer to
`?:help  esc:exit`. Every sub-mode below lives inside this one.

| Key | Action |
|---|---|
| `j` / `↓` / `Tab` | next pane |
| `k` / `↑` | previous pane |
| `g` / `G` | first / last |
| `Enter` / `l` / `Space` | jump to the selected pane and exit the mode |
| `/` | search |
| `n` | number jump |
| `t` | triage |
| `d` | terminate a pane |
| `m` / `M` | mark / unmark a pane, or clear every mark |
| `p` | toggle the preview |
| `r` | mark the previewed pane as read (only while the preview is up) |
| `?` | open the key help |
| `Esc` / `q` | exit the mode |

Any other key also exits the mode (a safety valve so you never get stuck with
keystrokes going nowhere).

The highlighted row always follows the pane you have focused, so nav mode starts on
the pane you are working in — except that leaving with `Esc` and coming back without
moving the focus resumes where you were browsing. While the mode is active the
sidebar borrows zellij's focus, so your working pane **keeps its frame but loses the
focused-pane highlight**. Leaving hands the focus back to the pane you came from, or
the one you jumped to — unless you moved the focus yourself mid-mode, in which case
it is left where you put it.

### Search (`/`)

The footer becomes a query input line (`/…`) and fuzzy-filters the tree against
pane name, owning tab name, and cwd. Matched characters are highlighted, which
doubles as a hint for which field matched (a cwd match shows on that row even if
`show_cwd` is disabled). With zero matches the list reads `no matches`.

Search is split into two vim-like states: **editing** and **navigating**. `/`
starts you in editing; `Esc` moves you to navigating while keeping the query. You
can tell them apart by the query colour (normal vs dimmed) and the footer hints
(`esc:browse  enter:jump` vs `j/k:move  ?:help  i:edit …`).

| Key | Editing | Navigating |
|---|---|---|
| `Enter` | jump to the selected row and exit nav mode (no-op with zero matches) | ← same |
| `Esc` | switch to navigating (the query is kept) | discard the query, back to nav mode |
| `↓` / `Tab`, `↑` / `Shift+Tab` | move the cursor within the results | ← same |
| `j` / `k` | append to the query | move the cursor within the results |
| `i` | append to the query | go back to editing (the query is kept) |
| `?` | append to the query | open the key help |
| `Backspace` | delete the last character | nothing |
| any other printable character | append to the query | nothing |
| `alt+m` / `alt+p` | mark / toggle the preview | ← same |

A key that editing has no meaning for (`←` / `→`, a function key, …) exits nav mode
entirely; in navigating it does nothing. Anything with a modifier other than
`alt+m` / `alt+p` exits nav mode from either state.

The query is discarded every time you leave search, so it always starts empty
next time. `cwd` is only known for panes that reported it via the hook — an
ordinary shell pane won't match on cwd.

### Number jump (`n`)

Every selectable pane is numbered — one running sequence across all tabs — in a
column on the left. Typing digits narrows by prefix and the jump happens the
moment one candidate is left; there is no `Enter` to press. Numbers are
zero-padded to the width of the total (`01`–`12` with 12 panes), so no number is a
prefix of another. Typing a number that doesn't exist clears the buffer.

| Key | Action |
|---|---|
| `0`-`9` | narrow the candidates, jumping as soon as one is left |
| `Backspace` | delete the last digit |
| `?` | open the key help |
| `Esc` | leave the sub-mode and go back to the tree (nav mode continues) |

The footer shows `n` followed by the digits typed so far, and only the numbers
still in the running stay lit. The number column is only there while the sub-mode
is active, so it never eats into the usual row layout.

### Triage (`t`)

The header changes to `▲ fujin  [tri]` and the list flattens, ignoring tab
boundaries. Only panes that carry a status are shown, ordered by urgency
(`error` > `blocked` > `working` > `done`; ties break by most recently changed),
each tagged with its owning tab name on the right. Panes that have never reported
a status and read (`idle`) ones are left out; command pane states join the same
ranking. An empty list reads `nothing to triage`.

| Key | Action |
|---|---|
| `j` / `↓` / `Tab`, `k` / `↑` / `Shift+Tab` | move the cursor |
| `g` / `G` | first / last |
| `Enter` | jump to the selected pane and exit the mode (no-op if the list is empty) |
| `m` / `M` | mark / unmark a pane, or clear every mark |
| `p` / `r` | toggle the preview / mark the previewed pane as read |
| `?` | open the key help |
| `Esc` | go back to the tree (nav mode continues) |

Leaving with `Esc` restores whichever row was selected before you entered (the
footer hint reads `esc:back` rather than `esc:exit` for that reason).

### Marks (`m` / `M`)

`m` marks the pane on the highlighted row, pressing it again unmarks it, and `M`
clears every mark. A marked row shows `✓` to the left of its state icon (the
column only appears in frames that have at least one mark, so it never eats into
the usual row layout).

- `m` / `M` in the tree and the triage list, `alt+m` in the filtered search results
- **Marks span tabs, and they survive leaving nav mode**
- Closing a pane drops just that pane's mark
- Running a termination clears them all; cancelling with `Esc` keeps them

For now, marks exist to give the termination below a batch of targets.

### Terminate a pane (`d`)

The footer becomes a confirmation prompt (`c:close k:kill x:kill+close`) and the
`▲` in the header turns the theme's error colour with it. Nothing happens until
you pick one of the three, and `Esc` backs out.

| Key | Action |
|---|---|
| `c` | close the pane, leaving its process alone |
| `k` | send `SIGKILL` to the pane's process |
| `x` | kill, then close the pane |
| `?` | open the key help |
| `Esc` | cancel and go back to the tree (nav mode continues) |

The targets are every [marked](#marks-m--m) pane, or just the selected one if
nothing is marked. With marks in play the prompt is prefixed with the count, as in
`3 panes  c:close k:kill x:kill+close`, and running it clears every mark. With no
targets at all, the prompt doesn't open.

All three are offered whatever the pane is running — an agent, a command pane, or
an unrelated shell. Killing a shell takes its children with it and zellij closes
the pane on its own; a command pane (`zellij run -- …`) stays on screen after its
command dies, which is what `x` is for.

### Preview (`p`)

A preview opens to the right of the sidebar showing the contents of the
highlighted pane. The focus does not move, so you can keep walking the list with
`j` / `k` and see what each pane is up to. Pressing `p` again — or jumping, or
leaving nav mode — closes it (inside the search sub-mode the key is `alt+p`).

What you get is a **snapshot taken when the selection moved**: the pane may keep
running behind it, but the preview will not change until you move again. Zellij only
hands plugins the plain text of a pane, so colours and bold are gone too — this is
for getting the gist, not a faithful reproduction.

Looking at a pane does not mark it read, since the focus never moves there. Press
`r` to say you have seen it (`done` / `blocked` / `error` only).

### Help (`?`)

The sidebar is 32 columns wide, which is not enough to spell out every key, so the
always-visible hints are limited to `?:help` and `esc:exit` in the footer. `?`
keeps the header in place, replaces the tree with the key list plus the status
icon legend, and turns the footer into `press any key to close`. Any key closes
it; that key is not acted on, so press it again if you meant it as a command. Nav
mode stays active the whole time.

The key list is specific to the mode you are in, while the status icon legend is
the same wherever you open it from. Anything that doesn't fit the screen gets the
same overflow markers as the list.

## Configuration

**Write settings on the alias definition (the `plugins` block in `config.kdl`).**
That is the only place fujin supports (→
[Only the alias route is supported](#only-the-alias-route-is-supported)). The list
below is exhaustive.

<!-- settings:begin -->
<!-- Generated from SETTINGS in repos/main/src/config.rs. Don't edit by hand; run `make readme` -->

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm" {
        show_cwd              "true"
        show_cwd_tilde        "true"
        show_deploy_animation "false"
        up_key                "Alt u"
        down_key              "Alt d"
        go_key                "Alt g"
        toggle_cwd_key        "Alt c"
    }
}
```

| Key | Value | Default | What it does |
| --- | --- | --- | --- |
| `show_cwd` | `"true"` / `"false"` | `false` | Show cwd under each pane row (only for panes with the hook set up) |
| `show_cwd_tilde` | `"true"` / `"false"` | `false` | Shorten the home directory in the cwd row to `~` (experimental) |
| `show_deploy_animation` | `"true"` / `"false"` | `true` | Play the deployment animation in the header when new agents appear |
| `up_key` | Key spelling (`"Alt u"` / `"alt+u"`) | unset (the hint is omitted) | Spelling of the key bound to `fujin_up` (footer hint only) |
| `down_key` | Key spelling (`"Alt u"` / `"alt+u"`) | unset (the hint is omitted) | Spelling of the key bound to `fujin_down` (footer hint only) |
| `go_key` | Key spelling (`"Alt u"` / `"alt+u"`) | unset (the hint is omitted) | Spelling of the key bound to `fujin_go` (footer hint only) |
| `toggle_cwd_key` | Key spelling (`"Alt u"` / `"alt+u"`) | unset (the hint is omitted) | Spelling of the key bound to `fujin_toggle_cwd` (footer hint only) |

<!-- settings:end -->

- Quote the values (`show_cwd "true"`). The property form (`show_cwd="true"`)
  means the same thing
- The only booleans accepted are `"true"` and `"false"` — not `1` or `yes`
- A value that can't be parsed shows in the footer as `!bad value: show_cwd`
  **for a few seconds after startup**, and that setting falls back to its default

### Footer key hints

`up_key` / `down_key` / `go_key` / `toggle_cwd_key` are **display-only** and bind
nothing. Bind the keys the usual way (see [Direct keys](#direct-keys-no-mode)) and
repeat them here — zellij doesn't pass the destination pipe name to the plugin, so
fujin cannot read the real bindings. Anything you leave out loses its hint.

Both `"Alt u"` and `"alt+u"` are accepted and both render as `alt+u`. A value that
can't be parsed is shown verbatim, so a typo stays visible instead of vanishing.
When every hint shares the same modifier it is lifted to the front, as in
`alt + › u:up  d:down  g:jump`.

### Only the alias route is supported

fujin only guarantees the setup where **settings live on the alias definition and
both the layout and the keybindings refer to it by that alias name**. Writing the
wasm path straight into the layout does work, but is not supported.

> [!WARNING]
> `MessagePlugin` matches its destination **by configuration as well as wasm path**.
> Put `show_cwd "true"` on the layout's plugin block but not on the keybinding and
> the two count as different plugins, so **the key never reaches the resident
> sidebar**. Worse, zellij **opens a new matching instance right there**. The fujin
> sidebar is excluded from focus cycling, so **the pane that creates cannot be
> closed**.

The alias route makes this impossible. Without it, you must copy the same
configuration into the layout and into **every** `MessagePlugin`.

## Agent state notifications

### Claude Code hooks

`setup.sh` copies `extras/claude-hooks/fujin-hook.sh` into
`~/.config/zellij/plugins/` and registers that path in `~/.claude/settings.json`.
By hand it looks like this:

```jsonc
{
  "hooks": {
    // add the same entry to every one of these events:
    // SessionStart, UserPromptSubmit, Stop, StopFailure,
    // SessionEnd, SubagentStart, SubagentStop, TaskCreated, TaskCompleted
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/fujin-hook.sh" }
        ]
      }
    ],
    // Notification is the only one that filters by matcher
    "Notification": [
      {
        "matcher": "permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog",
        "hooks": [
          { "type": "command", "command": "/path/to/fujin-hook.sh" }
        ]
      }
    ]
  }
}
```

Hooks take effect **from the next Claude Code session you start**. A Claude Code
running outside zellij, or with no sidebar up, is a complete no-op.

| Hook event | State transition |
|---|---|
| `SessionStart` | idle (register, reset counters) |
| `UserPromptSubmit` | working |
| `Notification` | blocked (message retained) |
| `Stop` | done — but stays working while background subagents are still running |
| `StopFailure` | error — not overwritten by a following `Stop` |
| `SessionEnd` | unregister |
| `SubagentStart` / `SubagentStop` | subagent count ±1; done when the last one stops after `Stop` |
| `TaskCreated` / `TaskCompleted` | incomplete task count ±1 |

### Other agents

The plugin listens on the pipe name `fujin_status` for the JSON below. Agents
other than Claude Code (codex, etc.) show up the same way as long as they send
this shape — **unverified**, in that it should work but hasn't been confirmed
against a real agent.

```bash
zellij pipe --name fujin_status -- '{
  "pane_id": '$ZELLIJ_PANE_ID',
  "agent": "codex",
  "event": "UserPromptSubmit",
  "cwd": "/path/to/project"
}'
```

`pane_id` is the `$ZELLIJ_PANE_ID` zellij assigns to each pane, `event` is a name
from the table above, and `cwd` / `detail` are optional.

> [!IMPORTANT]
> Don't pass the `--plugin` option. Doing so makes zellij launch the plugin if it
> isn't already running. Without it, the message only reaches a running plugin and
> is a harmless no-op otherwise.

The pipes meant for external use are `fujin_status`, the keybinding ones
(`fujin_up` / `_down` / `_go` / `_mode` / `_toggle_cwd`), and `fujin_dismiss` for
cleanup — `fujin_toggle_cwd` flips with no payload, or forces a value with
`true` / `false`. `fujin_sync_state` / `_read` / `_selection` / `_command` /
`_mark` / `_preview` / `_width` are an internal protocol for syncing between
instances; don't call them from outside.

## Troubleshooting

### The sidebar doesn't show up

Nearly always a layout that isn't in effect. Check that `config.kdl` has
`default_layout "fujin"` and that the layout spells out `new_tab_template`
(`setup.sh` reports whichever is missing). `zellij action dump-layout` shows what
is actually loaded.

If the space is reserved but **completely blank**, permission approval stalled: a
single unapproved permission leaves both the prompt and fujin's drawing off screen
(which also happens when you approved an older version and the required permissions
have grown since). Delete that wasm's entry from `permissions.kdl` in zellij's cache
directory and start a fresh session.

### Updated the wasm but the old behaviour persists

**Swapping the wasm does not reach running sessions.** Existing instances keep
running the old one, so start a fresh session.

### The keybinding (`Ctrl+y`, …) does nothing

Without the alias, the layout and the keybinding may disagree on configuration
(→ [Only the alias route is supported](#only-the-alias-route-is-supported)).

### The permission prompt keeps coming back

Approvals are recorded **per resolved absolute path**. Moving the file makes it a
new path.

### A floating sidebar got left behind

When the focused tab has no sidebar of its own, `Ctrl+y` makes fujin summon itself
as a temporary floating pane. It normally closes itself on `Esc` or on a jump, but
a stranded one is excluded from focus cycling and so unreachable by hand. This
sweeps them up:

```bash
zellij pipe --name fujin_dismiss
```

### It won't start, or behaves strangely

Clear zellij's cache and try again. That also drops the recorded approvals
(`permissions.kdl`), so the next launch asks again.

```bash
rm -rf ~/Library/Caches/org.Zellij-Contributors.Zellij   # macOS
rm -rf ~/.cache/zellij                                   # Linux
```

### Trying it without changing any config

It works as a floating pane. zellij's built-in plugin manager (`Ctrl+o` → `p`) can
also load it by path.

```bash
zellij action new-pane --floating --width 40 --height 20 -p "fujin"
```

## Appendix: configuring by hand

For setting things up without `setup.sh`, or folding parts into an existing
configuration. Keybindings are in [Keybindings](#keybindings) and hooks in
[Agent state notifications](#agent-state-notifications).

<details>
<summary>Placing the wasm and defining the alias</summary>

The wasm can live anywhere, but `~/.config/zellij` is the config directory on
every OS, so keeping it there makes the steps environment-independent.

```bash
mkdir -p ~/.config/zellij/plugins
curl -fsSL https://github.com/yo-goto/fujin/releases/latest/download/fujin.wasm \
  -o ~/.config/zellij/plugins/fujin.wasm
```

Defining an alias in `config.kdl` means the layout and the keybindings only ever
say `"fujin"`.

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}
```

zellij expands the `~` in `file:~/…` (and environment variables like `$HOME`).
**Relative paths are not allowed** — there is no cwd to resolve them against, so
use an absolute path, a `~` path, or `https://…`.

</details>

<details>
<summary>Keeping the sidebar resident (layout)</summary>

Embed the sidebar in the default layout's tab template
(`~/.config/zellij/layouts/fujin.kdl`):

```kdl
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="zellij:tab-bar"
        }
        pane split_direction="vertical" {
            pane size=32 borderless=true {
                plugin location="fujin"
            }
            pane
        }
        pane size=1 borderless=true {
            plugin location="zellij:status-bar"
        }
    }
    // write the same thing under new_tab_template
}
```

> [!CAUTION]
> Write `pane`, not `children`. `children` marks where the panes of the tabs this
> layout defines get slotted in, so a template-only layout with no `tab` node gets
> **nothing, producing a tab with zero terminals**. When that happens at session
> creation, zellij exits.

<!-- -->

> [!IMPORTANT]
> Write the same content under `new_tab_template` too. `default_tab_template` does
> fall back to the new-tab template, but **not for a session created by picking a
> layout in the session manager** (`Ctrl+o` → `w`). Writing both makes it
> independent of how the session started.

Then add `default_layout "fujin"` to `config.kdl`. You can also name the layout on
the `NewTab` keybinding
(`bind "n" { NewTab { layout "fujin"; } SwitchToMode "normal" }`).

The permissions requested on first load are `ReadApplicationState` /
`ChangeApplicationState` / `ReadCliPipes` / `InterceptInput` /
`MessageAndLaunchOtherPlugins` / `OpenTerminalsOrPlugins` / `ReadPaneContents`.

</details>

## License

MIT License (see `LICENSE`). `zellij-tile`, the one dependency, is MIT as well.
