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

Why the ancestor walk: Claude Code spawns `$EDITOR` from the project directory,
so the editor's cwd is already right. Codex spawns `$EDITOR` with cwd=`/tmp`,
but the Codex process itself still sits in the project — promptls walks up
the process tree (via `sysinfo`: `/proc` on Linux, `proc_pidinfo` on macOS)
until it finds a cwd outside the temp dir. No editor-side config needed for
either. An explicit root inside a temp dir is a hard error.

Supported: Linux (incl. WSL) and macOS. Windows is untested.

## Neovim (0.11+)

Attach only to `.md` files under the temp dir, never to ordinary Markdown:

```lua
vim.lsp.config("promptls", {
  cmd = { "promptls" },
  filetypes = { "markdown" },
  root_dir = function(bufnr, on_dir)
    local name = vim.api.nvim_buf_get_name(bufnr)
    if not name:match("%.md$") then return end
    -- Both CLIs use $TMPDIR if set, else /tmp (Node os.tmpdir / Rust temp_dir).
    -- Compare realpaths too: on macOS /tmp is a symlink to /private/tmp.
    local tmpdir = (os.getenv("TMPDIR") or "/tmp"):gsub("/*$", "/")
    local real_tmpdir = (vim.uv.fs_realpath(tmpdir) or tmpdir):gsub("/*$", "/")
    local real_name = vim.uv.fs_realpath(name) or name
    if vim.startswith(name, tmpdir) or vim.startswith(real_name, real_tmpdir) then
      on_dir(vim.fn.getcwd()) -- nominal; promptls resolves the real root itself
    end
  end,
})
vim.lsp.enable("promptls")
```

Then `Ctrl+G` in Claude Code / Codex → type `@src/co` → completion (nvim-cmp
picks it up via the `@` trigger, or `<C-x><C-o>`) → `K` for hover, `gd` to
jump, `Ctrl+o` to come back.

## Debugging

`RUST_LOG=debug promptls --root .` logs to stderr. Startup logs
`indexed N entries` via `window/logMessage` (`:LspLog` / `:checkhealth vim.lsp`).

## Non-goals (for now)

Prompt-quality linting, symbol completion inside backticks, `/`-command
completion. The scope is: make writing a one-shot prompt as fast as an IDE.
