# gridterm

[![CI](https://github.com/RuMont/gridterm/actions/workflows/ci.yml/badge.svg)](https://github.com/RuMont/gridterm/actions/workflows/ci.yml)
[![License: MIT-0](https://img.shields.io/badge/license-MIT--0-blue.svg)](LICENSE)

A native terminal workspace for macOS. gridterm lays your terminals out in a grid
and keeps a Git client, a file explorer and fuzzy file search beside them, all
following whichever terminal has the focus. It also talks to coding agents
(Claude Code, opencode, Copilot CLI, omp) so they can see what you are looking at
in their cell.

Written in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) (Zed's UI framework) and
[`alacritty_terminal`](https://github.com/alacritty/alacritty).

## Features

- **Terminal grid.** Terminals fill the window horizontally first, then vertically
  (up to 3×2 by default). Drag the separators to resize cells and rows (double-click
  to even them out), zoom one terminal with ⌘⇧↩, and move between them with
  ⌘1…⌘9 or ⌘⌥ + arrows.
- **Context that follows the active terminal.** The sidebar tools always work on the
  folder and repository of the focused terminal: its shell's current directory,
  re-read whenever the focus changes and every 500 ms. Pin it with 📌.
- **Git.** Status, staging by file, hunk or single lines (like `git add -p`), commit,
  undo the last local commit, revert a pushed commit, push and pull with live
  progress and cancel. The history is a compact graph of every local and remote
  branch and tag, and opens in a large window with full commit details, per-file
  diffs and links to the commit on GitHub, GitLab, Bitbucket or Azure DevOps.
  Diffs are syntax highlighted. Errors become actionable messages, gridterm asks
  before `push --set-upstream`, and it tells you when Git Credential Manager is
  waiting for you in its own window (which can end up behind gridterm).
- **Explorer and search.** A lazy file tree that honours `.gitignore`, content search
  powered by ripgrep's engine (⇧⌘F), and fuzzy file-name search in a floating
  palette (⌘P). Files open as tabs of the terminal they belong to.
- **Read-only viewer.** Syntax highlighting, line numbers, ⌘F, Git change marks next
  to the line numbers and in the scrollbar, and ⌘-click / F12 *go to definition*
  for TypeScript/JavaScript and Rust (follows imports, `tsconfig` aliases,
  `node_modules` and re-exports, without a language server).
- **Smart links.** ⌘-click a URL to open it, a `path:line` to open the file at that
  line, or a folder to reveal it in the Explorer. OSC 8 hyperlinks and URLs that
  wrap across rows work too.
- **Busy indicator.** A cell running a command (a dev server, tests, an agent) gets an
  orange frame and a dot on its tab; the dot pulses while the command prints.
- **Child terminals.** Right-click a cell's tabs → *New terminal in cell* to open
  more terminals that share the cell's files and agent context.
- **Coding agents.** Claude Code's `/ide` protocol and a built-in MCP server, with
  zero configuration. See [Coding agents](#coding-agents).
- **Open from anywhere.** Finder *Open With*, the Dock, `open -a gridterm` and a
  `gridterm open` command for editors and scripts.
- **Sessions.** The grid, each terminal's directory and its pinned tabs are restored
  on launch.
- **Rebindable shortcuts**, light and dark themes, and the sidebar on either side.

## Install

Requires macOS 13 or later on Apple Silicon (arm64).

1. Download `gridterm-<version>-arm64.zip` from the
   [latest release](https://github.com/RuMont/gridterm/releases/latest) and unzip it.
2. Move `gridterm.app` to `/Applications`.
3. The app is signed ad hoc, not with an Apple Developer ID, so macOS blocks it the
   first time ("cannot be opened" or "is damaged"). Allow it with:

   ```sh
   xattr -dr com.apple.quarantine /Applications/gridterm.app
   ```

   or right-click the app → *Open* → *Open*.

### Build from source

You need a stable Rust toolchain ([rustup](https://rustup.rs)) and the Xcode command
line tools.

```sh
git clone https://github.com/RuMont/gridterm.git
cd gridterm
cargo run --release        # binary at target/release/gridterm
scripts/bundle.sh          # app bundle at target/release/bundle/gridterm.app
```

## Getting started

On first launch gridterm asks for a folder for the first terminal (cancel to start
with an empty grid). While the grid has room, the next empty cell shows a **+**:
click it, press ⌘T or use the sidebar button to open another terminal in a folder
you pick.

Shrinking the grid in *Settings* never closes terminals silently: the ones that would
no longer fit are listed (those with running jobs in red) and only closed if you
confirm.

When you quit, the session is saved to `~/.config/gridterm/session.toml`: the grid,
each terminal's directory and pinned tabs, the active terminal and the sidebar
width.

Each cell is a terminal plus the files opened from it, shown as tabs. A file belongs
to the terminal that was active when you opened it (or the one you ⌘-clicked in).
Hidden terminals keep running; if one prints output, its tab shows a dot.

### Opening things from outside

- **Finder and the Dock.** gridterm appears under *Open With* for shell scripts
  (`.command`, `.tool`, `.sh`, `.bash`, `.zsh`…), Unix executables and folders. A
  script runs in a new terminal in its folder and leaves you at a shell with its
  output; a folder opens a terminal there. Dropping them on the Dock icon or running
  `open -a gridterm <path>` does the same.
- **As your default terminal.** macOS has no such setting; it goes by the app
  associated with each file type. In Finder, *Get Info* on a `.command` (and a
  `.sh`, or an executable) → *Open with* → gridterm → *Change All*. gridterm
  registers as an alternate handler, so Terminal stays the default until you do.
- **From scripts and editors.** `gridterm open [FOLDER] [-- COMMAND…]` opens a
  terminal in the folder (the current one by default) in the running gridterm,
  launching it if needed, and runs the command. A single argument after `--` is
  passed as a shell line: `gridterm open ~/project -- 'npm install && npm start'`.
  This works as the "external terminal" of editors such as VS Code or JetBrains IDEs.
  To put it on your `PATH`:

  ```sh
  ln -s /Applications/gridterm.app/Contents/MacOS/gridterm /usr/local/bin/gridterm
  ```

When the grid is full, anything opened from outside becomes a child terminal of the
active cell.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| ⌘T | New terminal (if there is room), in a folder picked in Finder |
| ⌘W | Close the visible file tab; on the terminal tab, close the terminal and its files |
| ⌘⇧] / ⌘⇧[ | Next / previous tab of the active cell |
| Esc | In a file: back to the terminal tab. In the sidebar: back to the active terminal |
| ⌘1 … ⌘9 | Focus terminal N |
| ⌘⌥ ← ↑ → ↓ | Move the focus around the grid |
| ⌘⇧↩ | Zoom the active terminal |
| ⌘B | Show or hide the sidebar |
| ⇧⌘G / ⇧⌘E | Git / Explorer |
| ⌘P / ⇧⌘F | Find a file by name / find in files (a single-line selection becomes the query) |
| ⌘, | Settings |
| ⌘-click or F12 on a symbol | Go to its declaration (in a file) |
| ⌘-click on a URL or path | Open it (see [Smart links](#features)) |
| ⌥⌘K | In a file: insert `@file#Lx-y` into the prompt of the agent in that cell |
| ⌘C / ⌘V / ⌘K | Copy / paste / clear the terminal. With only an image on the clipboard, ⌘V sends Ctrl-V to the foreground program, so Claude Code can paste it |
| ⇧PgUp / ⇧PgDn, ⌘Home / ⌘End | Scroll through the history |
| ⌥← / ⌥→, ⌥⌫ / ⌥⌦, ⌘← / ⌘→, ⌘⌫ | macOS line editing in the shell: word back/forward, delete word, start/end of line, delete line (with or without *Option as Meta*) |
| Tab / ⇧Tab | In the sidebar: move between its controls. In a terminal they go to the program |

Mouse:

- Drag, double-click or triple-click to select text. Programs that track the mouse
  (full-screen Claude Code, vim with `mouse=a`, tmux) receive the drag and select
  themselves; hold ⇧ to make gridterm select instead.
- Drag the sidebar edge to resize it; drag a grid separator to resize cells
  (double-click to even them out).
- Scroll over a cell's tabs to move through them when they don't fit.
- The terminal scrollbar appears near the right edge, while scrolling or while
  viewing the history. Click the track to jump, or drag the thumb.

Every shortcut except Esc, Tab and ⌘Q can be changed in *Settings → Keyboard
shortcuts…*: click a shortcut and press the new combination (it needs ⌘, ⌃ or ⌥, or
a function key). If another action in the same scope had it, it is taken from that
action and you are told. Changes apply at once and are saved to
`~/.config/gridterm/keymap.toml`, which only stores what differs from the defaults.

**File search palette:** ↩ or double-click opens the file in the active cell, ⇧↩
inserts its path into the active terminal, ⌥↩ `cd`s to its folder, ⌘↩ reveals it in
the Explorer, ⌘⇧C copies the path, Esc closes. File-name matches rank above matches
on a folder of the path, so `event` lists `ui/event_handler.rs` before
`events/mod.rs`.

**Explorer:** a click previews a file (an italic tab the next click replaces), a
double-click opens a pinned tab. Space previews, ↩ opens, ⌥↩ inserts the path into
the terminal. Clicking a content-search result opens the file at that line.

## Coding agents

### Claude Code `/ide`

gridterm acts as an IDE for Claude Code, with the same protocol VS Code and the
JetBrains IDEs use (undocumented; tested with Claude Code 2.1.331). A `claude`
started in a cell connects on its own, because every shell gets
`CLAUDE_CODE_SSE_PORT`; `/ide` also lists it as "gridterm".

Claude then knows which file is open **in its cell** (the last file tab shown, even
after you switch back to the terminal to type) and which lines are selected
(`⧉ 3 lines selected`). Tabs in other cells and the terminal's text are never
shared. Edits are still approved in the terminal: gridterm does not implement the
IDE diff view.

The server listens only on `127.0.0.1`, uses a random token, and publishes
`~/.claude/ide/<port>.lock` with 0600 permissions, removed on exit.

### MCP server

Agents started in a cell get gridterm's MCP tools **without any setup** (no
`mcp add`). The tools are read-only and always answer about the agent's own cell:

- `get_selection`: the file shown in the cell and the selected lines (1-based, with
  their text).
- `get_cell_context`: working directory, repository and branch, running processes,
  the file shown and the open tabs.

| Agent | How it gets the tools |
|---|---|
| Claude Code 2.1.331 | `CLAUDE_CODE_PLUGIN_DIRS`, pointing at a plugin gridterm generates |
| omp 19.1.1 | `--plugin-dir`, added by the zsh integration |
| Copilot CLI 1.0.115 | `--additional-mcp-config`, added by the zsh integration |
| opencode 2.2.5 | No MCP: it uses the `/ide` protocol like Claude Code, so the selection reaches it directly (shown in its status bar at about 100 columns or more, e.g. with the cell zoomed) |

The zsh integration uses the same technique as VS Code and Ghostty: gridterm starts
zsh with its own `ZDOTDIR`, whose `.zshenv` restores yours and loads **your** files
as usual, untouched. It then defines `omp` and `copilot` functions that add the
option only when launching the agent, never to subcommands (`omp config`,
`copilot mcp`…). Your own `omp` or `copilot` definitions win. bash and fish have no
integration, so only Claude Code and opencode work there.

`gridterm mcp` (the same executable) is the MCP server. It asks the app over a
private Unix socket (`GRIDTERM_SOCKET`), and the app identifies the calling process
through the OS, not through anything the client claims. An agent can therefore only
see its own cell, and outside gridterm the tools answer "not running inside a
gridterm cell".

Agents' global configuration (`~/.claude.json`, `~/.omp/agent/mcp.json`,
`~/.copilot/mcp-config.json`, opencode's) is never modified; generated files live in
`~/.config/gridterm/agents/`.

Both integrations can be turned off in *Settings → Agents* or in the configuration
file (`[agents] claude_ide = false`, `mcp = false`).

## Configuration

`~/.config/gridterm/config.toml`. Every field is optional, and *Settings* (⌘,) writes
the same file. Defaults:

```toml
[grid]
max_cols = 3
max_rows = 2

[terminal]
# shell = "/bin/zsh"          # default: $SHELL
font_family = "Menlo"
font_size = 13.0
line_height = 1.25
scrollback_lines = 10000
option_as_meta = false        # off: on non-US layouts Option types @ # [ ] { } |
dim_inactive = true
cursor_blink = true

[ui]
theme = "dark"                # or "light"
sidebar_side = "left"         # or "right"
sidebar_width = 300.0
sidebar_visible = true

[git]
pull_rebase = false
log_limit = 200

[search]
max_content_results = 2000    # lines shown by content search
max_indexed_files = 300000    # cap for the file-name index

[agents]
claude_ide = true
mcp = true
```

Changed shortcuts go to `keymap.toml` (action id → key combinations; an empty list
unbinds the action):

```toml
[bindings]
show_git = ["cmd-g"]
clear = []
```

Environment variables: `GRIDTERM_CONFIG_DIR` replaces `~/.config/gridterm`, and
`GRIDTERM_IDE_LOCK_DIR` replaces `~/.claude/ide`.

## How it works

gridterm is a Cargo workspace: a GPUI application on top of independent libraries
that know nothing about each other or, except `terminal`, about the UI.

```
crates/
  app/         the gridterm binary: workspace (grid + active context), sidebar panels
               (git, explorer, search, settings), file viewer, go-to-definition
  terminal/    PTY + alacritty_terminal, its GPUI view, keys → xterm sequences,
               links, process introspection (cwd via libproc)
  git_client/  git2 for reads and the index; the git CLI for commit, revert, push and
               pull (so hooks, signing and credential helpers just work); history graph
  fs_tools/    lazy tree (ignore), fuzzy index (nucleo), content search (ripgrep's
               grep crates), debounced watchers
  agents/      Claude Code /ide server, MCP server over stdio, the app's Unix socket
  layout/      pure grid layout and navigation
  config/      TOML configuration and keymap
```

- **Nothing blocks the UI thread.** Git, searches, file loading and syntax
  highlighting run in the background; searches can be cancelled. Highlighting uses
  syntect with gridterm's own off-thread parser (the library's parsed small files
  inline and froze the window for 75–110 ms).
- **Active context.** The current directory comes from the foreground process group
  of the terminal (`tcgetpgrp` + `proc_pidinfo`); the repository root is found by
  walking up to a `.git` with a few `stat` calls. Panels reload only when that root
  changes.
- **Watchers instead of polling.** The Git panel watches `.git` (`index`, `HEAD`,
  `refs`) and the non-ignored working tree; the Explorer re-reads loaded folders
  that change; the file index updates on create, delete and rename. All debounced.
- **File search on demand.** The index is built only while the search is visible, so
  a terminal in `~` costs nothing until you search. Results refresh at most every
  50 ms, and typing is debounced by 250 ms.
- **Rendering.** The shell is forked off the UI thread, PTY output is batched (at most
  one refresh every 4 ms), idle terminals are cached views that don't repaint, only
  visible lines are painted, and every list is virtualized.
- **Processes.** Each terminal runs your shell directly as a login shell
  (`$SHELL -l`), without `/usr/bin/login`, so if gridterm dies its shells die with it
  and nothing is orphaned (the price: no "Last login" line). Closing a terminal or
  quitting sends SIGHUP, then SIGKILL after 1 s.
- **Opening from outside.** macOS hands everything to the app as `file://` URLs.
  `gridterm open` drops a request into `~/.config/gridterm/requests/` (a private
  folder) and opens it; the app only accepts recent requests from that folder, owned
  by the same user, and deletes them once read.
- **Viewer limits.** Binary files (a NUL in the first 8 KB) and files over 64 MB show
  a notice; very long files show their first 50,000 lines / 4 MB.

## Building and testing

```sh
cargo test --workspace        # unit and integration tests
scripts/e2e.sh [out_dir]      # end-to-end suite in the real app → out_dir/RESULTS.md
```

The end-to-end suite builds the app with the `e2e` feature and runs it in an
isolated environment (temporary configuration, zsh without user rc files). A runner
inside the app creates a fixture (a repository with a bare remote, a second clone, a
folder outside any repository and 1,000 commits of history), drives the UI with real
keyboard and mouse events and checks the state, saving a screenshot and a dump for
every failure. It also relaunches the app to check session restore and runs the
release-mode performance tests and benchmarks. `GRIDTERM_E2E_STOP_AFTER=<id>` stops
after a given case. It needs a display, so it runs locally rather than in CI.

CI runs clippy (with and without `--features e2e`, warnings denied) and the
workspace tests on macOS. Releases are built by GitHub Actions when a `vX.Y.Z` tag
is pushed; their notes list the commits since the previous version, grouped by
[Conventional Commits](https://www.conventionalcommits.org/) type.
`scripts/release-local.sh` runs the same pipeline locally.

## Known limitations

- **macOS on Apple Silicon only.** Process introspection uses macOS APIs, and
  releases are built for arm64.
- **Unsigned by Apple.** Releases are signed ad hoc, hence the quarantine step above;
  avoiding it needs a Developer ID certificate and notarization.
- **Cold start** takes 190–390 ms to the first frame depending on load. gridterm's own
  code accounts for about 6 ms; the rest is AppKit and GPUI (an empty GPUI app takes
  225–268 ms on the same machine). The first launch after installing or updating pays
  about 130 ms more to compile Metal shaders, and macOS's security scan of a new
  binary adds 0.7–1.2 s once.
- **Input methods** are verified against GPUI's `InputHandler` contract, but
  composing with a real system IME can't be automated; it is worth trying by hand
  (´ + e, ñ, Japanese).
- **The viewer is read-only**; editing happens in your editor or in the terminal.

## License

[MIT No Attribution](LICENSE) (MIT-0): use, copy, modify and distribute gridterm for
any purpose, with or without credit.
