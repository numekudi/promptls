# promptls

A tiny language server for **one-shot prompt files** — the buffers Claude Code
(`$TMPDIR/claude-prompt-*.md`) and Codex (`/tmp/.tmpXXXX.md`) open in `$EDITOR`
on `Ctrl+G`. Rule of thumb: any `.md` directly under the temp dir is a prompt.

The prompt file lives in `$TMPDIR`, but everything you want to reference is in
your project. promptls resolves `@path` references against the **project root**
(the directory the editor was launched from), not the temp dir.

## Features

| Feature | What it does |
|---|---|
| Completion | `@` triggers fuzzy path completion over the whole repo (`@src/comp/but` → `src/components/Button.tsx`). Respects `.gitignore`, includes dotfiles, skips `.git`. Directories insert with a trailing `/` and `/` re-triggers completion. |
| Diagnostics | `@path` that does not exist under the root → warning (catches typos before the model sees them). |
| Hover | File: first 30 lines. Directory: listing. |
| Go to definition | Jump to the referenced file. `@src/foo.rs:42` jumps to line 42. |

Reference syntax: `@` at a word boundary followed by a path; ends at whitespace or
`( ) [ ] { } < > " ' \` , |`. Trailing `. , ; : ! ?` are ignored, so
"see @src/main.rs." works. `user@example.com` is not a reference.

## Install

```sh
cargo install --path .
```

## Root resolution

`--root <dir>` > `$PROMPTLS_ROOT` > **nearest ancestor process whose cwd is
outside the temp dir**.

Both Claude Code and Codex spawn `$EDITOR` from the project directory, so the
editor's cwd is normally already right and the walk stops immediately. The
ancestor walk (via `sysinfo`: `/proc` on Linux, `proc_pidinfo` on macOS) is a
safety net for wrappers/multiplexers that start the editor from somewhere else.
An explicit root inside a temp dir is a hard error.

Supported: Linux (incl. WSL) and macOS. Windows is untested.

## Neovim (0.11+)

Attach only to `.md` files under the temp dir, never to ordinary Markdown:

```lua
vim.lsp.config("promptls", {
  cmd = { "promptls" },
  filetypes = { "markdown" },
  -- Attach only to temp-dir .md files (the Ctrl+G buffers), not ordinary Markdown.
  root_dir = function(bufnr, on_dir)
    local name = vim.api.nvim_buf_get_name(bufnr)
    if not name:match("%.md$") then return end
    -- Both CLIs use $TMPDIR if set, else /tmp (Node os.tmpdir / Rust temp_dir).
    -- Compare realpaths too: on macOS /tmp is a symlink to /private/tmp.
    local tmpdir = (os.getenv("TMPDIR") or "/tmp"):gsub("/*$", "/")
    local real_tmpdir = (vim.uv.fs_realpath(tmpdir) or tmpdir):gsub("/*$", "/")
    local real_name = vim.uv.fs_realpath(name) or name
    if vim.startswith(name, tmpdir) or vim.startswith(real_name, real_tmpdir) then
      on_dir(vim.fn.getcwd())
    end
  end,
})
vim.lsp.enable("promptls")
```

Then `Ctrl+G` in Claude Code / Codex → type `@src/co` → completion (nvim-cmp
picks it up via the `@` trigger, or `<C-x><C-o>`) → `K` for hover, `gd` to
jump, `Ctrl+o` to come back.

Tip for nvim-cmp users: drop the `path` source in these buffers, otherwise it
lists the temp dir (where the buffer lives) next to promptls's results:

```lua
-- in your LspAttach handler
if client.name == "promptls" then
  require("cmp").setup.buffer({ sources = { { name = "nvim_lsp" }, { name = "buffer" } } })
end
```

## Keeping the CLI output visible while editing

Claude Code hands the whole screen to `$EDITOR` unless the editor's basename
contains a GUI-editor name (`code`, `cursor`, `subl`, `gedit`, ...). A wrapper
named e.g. `nvim-code` that opens Neovim in a side pane (tmux split or Windows
Terminal `wt.exe split-pane`) and blocks until it closes lets you read the
model's last output while writing the next prompt:

```sh
alias claude='VISUAL=nvim-code EDITOR=nvim-code claude'
```

See the author's dotfiles for the wrapper; it is not part of promptls.

## Debugging

`RUST_LOG=debug promptls --root .` logs to stderr. Startup logs
`indexed N entries` via `window/logMessage` (`:LspLog` / `:checkhealth vim.lsp`).

## Non-goals (for now)

Prompt-quality linting, symbol completion inside backticks, `/`-command
completion. The scope is: make writing a one-shot prompt as fast as an IDE.
