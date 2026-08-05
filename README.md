<div align="center">
  <img src="assets/logo.svg" alt="fujin logo" width="180" />
</div>

# fujin

A sidebar plugin for zellij. It lists tabs > panes in a vertical tree, visualizes
the state of AI agents (Claude Code, etc.) running in each pane, and lets you
jump to them with global keybindings.

**Zellij, plus eyes for your agents.**

> [!NOTE]
> The name comes from the Japanese word 布陣 (*fujin*), "to deploy troops" /
> "to arrange a formation" — treating your panes as a formation to arrange and
> oversee at a glance. To English speakers, `fujin` also reads as 風神 (*fūjin*),
> the Japanese god of wind — pairing the stillness of forming up with the motion
> of wind sweeping across your panes to watch over them.

```text
▸ 1 scheme
    nu
    koka
▾ 2 zeli-c                ← active tab
    nvim
  » claude +2             ← working, 2 subagents running
▸ 3 review
  ◆ claude                ← waiting for input (needs attention)
```

- **Overview** — every tab and pane in the session, always visible in the sidebar
- **State at a glance** — an icon tells you whether each agent is working,
  waiting on you, or finished
- **Jump** — reach any pane with a couple of keystrokes or a single click, while
  your focus stays in your working pane (with fuzzy search over pane name, tab
  name, and cwd)

## Status icons

| Icon | State | Meaning |
|---|---|---|
| `»` | working | processing a prompt |
| `◆` | blocked | waiting for permission / input (needs attention) |
| `●` | done | turn finished (unread) |
| `✕` | error | API error / tool failure |
| `○` | idle | agent starting up / waiting |
| `+N` | — | number of active subagents |
| `[N]` | — | number of incomplete tasks |

`done` / `blocked` / `error` are **cleared automatically once you focus that
pane** (a read-receipt model). Only the ones you haven't attended to stay lit.

## Requirements

- zellij 0.44 or later
- Rust + `rustup target add wasm32-wasip1` (build time only)
- `jq` (for the Claude Code hook)

## Install

### 1. Build and place the wasm

The wasm can live anywhere, but `~/.config/zellij` is the config directory on
every OS, so keeping it there makes the setup steps environment-independent:

```bash
make install   # release build, copied to ~/.config/zellij/plugins/fujin.wasm
```

To put it somewhere else: `make install PLUGIN_DIR=/path/to/plugins`.

### 2. Define an alias

In `~/.config/zellij/config.kdl`:

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm"
}
```

- **`~` (and env vars like `$HOME`) in `file:~/…` are expanded by zellij**, so
  you don't need to hardcode your home directory name
- From here on, layouts and keybindings only need to say `"fujin"`. Besides
  keeping the path in one place, this **structurally prevents a whole class of
  configuration-mismatch bugs** (see [Configuration](#configuration))
- The alias's `location` **cannot be a relative path**. cwd expansion doesn't
  apply here, so use an absolute path, a `~`-prefixed path, or `https://…`

## Setup

### 1. Keep the sidebar resident (layout)

Embed the sidebar in the default layout's tab template. Example
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
    // put the same content in new_tab_template too (see below)
}
```

> [!CAUTION]
> Use `pane`, not `children`. `children` is a marker meaning "insert this
> layout's own tab panes here." A template-only layout with no `tab` node has
> nothing to insert, so you'd end up with **a tab that has zero terminal panes**.
> If that happens at session creation, zellij just exits.

<!-- -->

> [!IMPORTANT]
> Write the same content into `new_tab_template` as well. `default_tab_template`
> is documented to fall back to the "new tab template," but **that fallback
> doesn't kick in when a session is created via the session manager
> (`Ctrl+o` → `w`) with a chosen layout** — you get zellij's built-in default
> instead. Writing both makes it work regardless of how the tab was created.

If you want it to work reliably no matter the launch path, you can also specify
the layout on the `NewTab` keybinding itself:

```kdl
bind "n" {
    NewTab { layout "fujin"; }
    SwitchToMode "normal"
}
```

And in `~/.config/zellij/config.kdl`:

```kdl
default_layout "fujin"
```

To try it out without making it resident, it also works as a floating pane:

```bash
zellij action new-pane --floating --width 40 --height 20 -p "fujin"
```

On first load you'll see a permission prompt — focus the pane and press `y` to
approve (required permissions: `ReadApplicationState` / `ChangeApplicationState` /
`ReadCliPipes` / `InterceptInput` / `MessageAndLaunchOtherPlugins` /
`OpenTerminalsOrPlugins`). Approval is recorded per absolute path of the
expanded wasm, so **moving the wasm means re-approving**.

The sidebar's width can be resized with zellij's standard resize keys
(`Ctrl+n`, etc.) just like any other pane.

### 2. Global keybindings (jump feature)

Add these to the keybinds block in `~/.config/zellij/config.kdl` (e.g.
`shared_except "locked"`). There's a nav-mode approach and a direct-key
approach, and you can use both together.

#### Nav mode (recommended)

Feels like zellij's own `Ctrl+p` → pane mode. `config.kdl` only needs **one
entry key**; the keys inside the mode are interpreted by the plugin itself:

```kdl
bind "Ctrl y" {
    MessagePlugin "fujin" {
        name "fujin_mode"
        floating true
    }
}
```

> [!IMPORTANT]
> Keep `floating true`. When a session has no fujin instance at all (a session
> created without the layout, for example), this pipe has no recipient, so zellij
> launches fujin itself. By default it opens tiled — splitting your working pane
> and landing on the right at an arbitrary width — and the plugin cannot fix that
> from the inside. With `floating true` it opens floating, and fujin moves itself
> to the same left edge and width as the resident sidebar (closing on `Esc` or on
> a jump). It has no effect when fujin is already running.

`Ctrl+y` is unused by zellij's defaults; feel free to pick something else if
it's taken (`p`/`t`/`n`/`h`/`s`/`o`/`q`/`g` are already spoken for by default).
Because the mode's keymap lives in the plugin, **you don't have to give up a
built-in mode**, and adding more keys never requires touching `config.kdl`.
See [Usage](#usage) for what the mode does.

#### Direct keys (no mode)

If you'd rather move with a single keystroke:

```kdl
bind "Alt Up" {
    MessagePlugin "fujin" { name "fujin_up"; }
}
bind "Alt Down" {
    MessagePlugin "fujin" { name "fujin_down"; }
}
bind "Alt g" {
    MessagePlugin "fujin" { name "fujin_go"; }
}
```

> [!CAUTION]
> Don't bind `Alt Enter`. Claude Code's Shift+Enter relies on a terminal-side
> setting (`Shift+Return -> ESC CR`, installed by `/terminal-setup`), and zellij
> interprets that `ESC CR` as `Alt Enter`. Claiming it breaks Shift+Enter's
> newline from reaching the pane.

<!-- -->

> [!NOTE]
> Don't use `MessagePluginId`. The sidebar launches one instance per tab,
> so targeting by ID causes key collisions. A `MessagePlugin` addressed by alias
> (or URL) reaches every instance instead.

### 3. Claude Code hooks (state notifications)

Register `extras/claude-hooks/fujin-hook.sh` as a hook. Add it to `hooks` in
`~/.claude/settings.json`:

```jsonc
{
  "hooks": {
    // Add the same entry for all of the following events:
    // SessionStart, UserPromptSubmit, Stop, StopFailure, PostToolUseFailure,
    // SessionEnd, SubagentStart, SubagentStop, TaskCreated, TaskCompleted
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "/path/to/extras/claude-hooks/fujin-hook.sh" }
        ]
      }
    ],
    // Notification is the only one that needs a matcher to narrow the type
    "Notification": [
      {
        "matcher": "permission_prompt|agent_needs_input|idle_prompt|elicitation_dialog",
        "hooks": [
          { "type": "command", "command": "/path/to/extras/claude-hooks/fujin-hook.sh" }
        ]
      }
    ]
  }
}
```

- The hook only takes effect for **newly started** Claude Code sessions
- It's a no-op for Claude Code sessions running outside zellij (a plain
  terminal)
- It's also a complete no-op when the sidebar isn't running (no side effects)

## Usage

Everything works while your focus stays in your working pane — you never need to
focus the sidebar itself (in fact it's excluded from focus cycling entirely).

### Click to jump

Left-clicking a pane row in the sidebar jumps to that pane, with no mode to
enter. A pane in another tab brings that tab along; a floating pane brings up
the floating layer. Clicking a tab heading, or the empty space below the list,
does nothing.

If nothing happens, check that zellij's `mouse_mode` is still enabled in
`~/.config/zellij/config.kdl` (it is on by default).

### Nav mode

Pressing `Ctrl+y` (the key bound above) changes the sidebar header to
`[NAV]  ?:help  esc:exit`, and the following keys become active:

| Key | Action |
|---|---|
| `j` / `↓` / `Tab` | next pane |
| `k` / `↑` | previous pane |
| `g` / `G` | first / last |
| `1`-`9` | jump straight to the nth item and exit the mode |
| `Enter` / `l` / `Space` | jump to the selected pane and exit the mode |
| `/` | enter search sub-mode (below) |
| `?` | open the full key help (below) |
| `Esc` / `q` | exit the mode |

Any other key also exits the mode (a safety valve so you never get stuck with
keystrokes going nowhere).

The highlighted row always follows the pane you have focused, so nav mode starts
on the pane you are working in. The one exception: if you leave with `Esc` and
come back without moving the focus, it resumes where you were browsing.

### Search (`/` inside nav mode)

Pressing `/` while in nav mode turns the header into a query input line
(`/…▏`) and fuzzy-filters the tree against pane name, owning tab name, and
cwd. Matched characters are highlighted, and the highlight location doubles
as a hint for which field matched (a cwd match is shown on that row even if
`show_cwd` is disabled).

| Key | Action |
|---|---|
| printable characters | append to the query (nav mode's single-letter shortcuts are all disabled while searching) |
| `Backspace` | delete the last character of the query |
| `↓` / `Tab`, `↑` / `Shift+Tab` | move the cursor within the filtered results |
| `Enter` | jump to the selected row and exit nav mode entirely (no-op if there are zero matches) |
| `?` | open the full key help (below; `?` is the one character that does not go into the query) |
| `Esc` | discard the query and return to nav mode (press `Esc` again to exit the mode) |

The query is discarded every time you leave search, so it always starts empty
next time. `cwd` is only known for panes that reported it via the hook — an
ordinary shell pane won't match on cwd.

### Help (`?` inside nav mode)

The sidebar is 32 columns wide, which is not enough to spell out every key, so
the always-visible hints are limited to `?:help` and `esc:exit` in the header.
Pressing `?` replaces the whole sidebar with the key list; any key closes it and
brings back whatever was on screen before (nav mode or the search sub-mode). The
key you press to close is not acted on, so press it again afterwards if you meant
it as a command. Nav mode stays active the whole time.

### Event → state mapping

| Hook event | State transition |
|---|---|
| `SessionStart` | idle (register, reset counters) |
| `UserPromptSubmit` | working |
| `Notification` | blocked (message retained) |
| `Stop` | done — but stays working while background subagents are still running |
| `StopFailure` / `PostToolUseFailure` | error |
| `SessionEnd` | unregister |
| `SubagentStart` / `SubagentStop` | subagent count ±1; done when the last one stops after `Stop` |
| `TaskCreated` / `TaskCompleted` | incomplete task count ±1 |

## Configuration

**Write settings on the alias definition (the `plugins` block in `config.kdl`).**

```kdl
plugins {
    fujin location="file:~/.config/zellij/plugins/fujin.wasm" {
        show_cwd "true"   // show cwd on each pane row (default: false)
    }
}
```

Since `cwd` comes from the hook payload, it's only shown for agent panes that
have the hook configured.

### Don't put settings only on the layout side

> [!WARNING]
> `MessagePlugin` destination matching is done on **the wasm path plus its
> configuration**, not the path alone. If you write `show_cwd "true"` only on
> the layout's plugin block and not on the keybindings, the two are treated as
> different plugins, and **keys stop reaching the resident sidebar**. Worse,
> zellij doesn't just fail silently — it **opens a brand new instance on the
> spot that matches the configuration** (observed in practice: one keypress adds
> one more plugin pane). Since fujin's sidebar calls `set_selectable(false)`,
> **the resulting pane can't be closed by the user or the CLI**.

Sticking to the alias means the layout and the keybindings both resolve to the
same definition, so this mismatch can't happen. If you skip the alias, you
need to copy the same configuration onto the layout and **every**
`MessagePlugin` call by hand.

## Wire protocol (supporting other agents)

The plugin listens on the pipe name `fujin_status` for JSON like the following.
Agents other than Claude Code (codex, etc.) will show up the same way as long
as they send this shape:

```bash
zellij pipe --name fujin_status -- '{
  "pane_id": '$ZELLIJ_PANE_ID',
  "agent": "codex",
  "event": "UserPromptSubmit",
  "cwd": "/path/to/project"
}'
```

- `pane_id`: the `$ZELLIJ_PANE_ID` zellij assigns to each pane
- `event`: an event name from the mapping table above
- `cwd` / `detail`: optional

The only pipe names meant for external use are `fujin_status` and the
keybinding ones, `fujin_up` / `_down` / `_go` / `_mode`. `fujin_sync_state` /
`_read` / `_selection` are an internal protocol for syncing between instances
— don't call them from outside.

> [!IMPORTANT]
> Don't pass the `--plugin` option. Doing so makes zellij launch
> the plugin if it isn't already running. Without it, the message is only
> delivered to a running plugin, and the call is a harmless no-op when nothing
> is running.

## Development

Development tasks live in the Makefile. Run `make check`
(`fmt-check` → `lint` → `test`) before committing.

Detailed build/test/manual-verification steps and the implementation gotchas
are kept separately under `docs/dev/`.

## License

MIT License (see `LICENSE`). The `zellij-tile` dependency is also MIT.
