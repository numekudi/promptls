//! `promptls setup` — the post-install guide.
//!
//! `cargo install` cannot run hooks or print anything beyond "Installed
//! package", so the editor wiring (Neovim config, the tmux side-pane wrapper
//! and the shell aliases that select it) is delivered as a subcommand instead.
//! The Neovim snippet and the wrapper script are embedded from `contrib/` so
//! the guide, the shipped files and the README cannot drift apart.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

/// Neovim 0.11+ config that attaches promptls only to temp-dir `.md` buffers.
pub const NVIM_CONFIG: &str = include_str!("../contrib/promptls.lua");

/// `$VISUAL`/`$EDITOR` wrapper: Neovim in a tmux side pane, blocking until it
/// closes. Its basename must contain a GUI-editor name (here `code`) — that is
/// the only signal Claude Code uses to decide *not* to take over the screen.
pub const WRAPPER: &str = include_str!("../contrib/nvim-code");
pub const WRAPPER_NAME: &str = "nvim-code";

/// Shell aliases that route only the AI CLIs through the wrapper, leaving
/// `$EDITOR` for git etc. untouched.
pub const ALIASES: &str = "alias claude='VISUAL=nvim-code EDITOR=nvim-code claude'\n\
                           alias codex='VISUAL=nvim-code EDITOR=nvim-code codex'\n";

/// The full guide as printed by `promptls setup`.
pub fn guide() -> String {
    format!(
        "\
# promptls setup

## 1. Neovim (0.11+) — add to init.lua

{NVIM_CONFIG}## 2. Keep the CLI output visible while editing (optional, tmux)

Install the bundled wrapper (Neovim in a tmux side pane; plain nvim outside tmux):

    promptls setup --install-wrapper          # -> ~/.local/bin/{WRAPPER_NAME}
    promptls setup --install-wrapper DIR      # -> DIR/{WRAPPER_NAME}

Then route the AI CLIs through it (add to ~/.zshrc / ~/.bashrc):

{ALIASES}
The name must contain \"code\": Claude Code only skips the alternate screen
when the editor's basename contains a GUI-editor name.

## 3. Try it

Ctrl+G in Claude Code / Codex, type `@src/co`, get completion; `K` hover,
`gd` jump. `RUST_LOG=debug promptls --root .` if something looks off.
"
    )
}

/// Write the wrapper into `dir` (created if missing) with the exec bit set.
/// Refuses to clobber a differing file unless `force`, so a locally patched
/// copy is never silently replaced.
pub fn install_wrapper(dir: &Path, force: bool) -> anyhow::Result<PathBuf> {
    let dest = dir.join(WRAPPER_NAME);
    match std::fs::read_to_string(&dest) {
        Ok(existing) if existing == WRAPPER => return Ok(dest),
        Ok(_) if !force => bail!(
            "{} exists and differs from the bundled wrapper; pass --force to overwrite",
            dest.display()
        ),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", dest.display())),
    }
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&dest, WRAPPER).with_context(|| format!("writing {}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod {}", dest.display()))?;
    }
    Ok(dest)
}

/// Default wrapper location: `$HOME/.local/bin`, which is on PATH on most
/// Linux/macOS setups (same place `cargo install` users typically already have).
pub fn default_bin_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("$HOME is not set; pass --install-wrapper DIR")?;
    Ok(PathBuf::from(home).join(".local/bin"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_basename_satisfies_claude_code_gui_check() {
        // Claude Code: e0S=["code","cursor","windsurf","codium","subl","atom","gedit","notepad++","notepad"]
        assert!(WRAPPER_NAME.contains("code"));
        assert!(WRAPPER.starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn readme_matches_embedded_snippets() {
        // README shows the same config/aliases; keep them in lockstep with contrib/.
        let readme = include_str!("../README.md");
        assert!(readme.contains(NVIM_CONFIG.trim_end()), "README nvim snippet drifted from contrib/promptls.lua");
        for line in ALIASES.lines() {
            assert!(readme.contains(line), "README is missing alias line: {line}");
        }
    }

    #[test]
    fn install_is_idempotent_and_refuses_to_clobber() {
        let dir = tempfile::tempdir().unwrap();
        let dest = install_wrapper(dir.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), WRAPPER);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&dest).unwrap().permissions().mode() & 0o111, 0o111);
        }
        // Same content again: no-op.
        install_wrapper(dir.path(), false).unwrap();
        // Differing content: refused without --force, replaced with it.
        std::fs::write(&dest, "patched").unwrap();
        assert!(install_wrapper(dir.path(), false).is_err());
        install_wrapper(dir.path(), true).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), WRAPPER);
    }
}
