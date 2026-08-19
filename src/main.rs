//! promptls — a language server for one-shot prompt files (e.g. the
//! `claude-prompt-*.md` buffers Claude Code opens on Ctrl+G).
//!
//! Provides fuzzy `@path` completion, missing-path diagnostics, hover
//! previews and go-to-definition, all relative to the *project* root rather
//! than the temp directory the prompt file lives in.

mod index;
mod refs;
mod server;
mod setup;
mod text;

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use tower_lsp::{LspService, Server};

#[derive(Parser, Debug)]
#[command(name = "promptls", version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Project root used to resolve `@path` references.
    /// Precedence: --root, then $PROMPTLS_ROOT, then the nearest ancestor
    /// process whose cwd is outside the temp dir.
    #[arg(long)]
    root: Option<PathBuf>,

    /// Stop indexing after this many filesystem entries.
    #[arg(long, default_value_t = 200_000)]
    max_entries: usize,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print the editor setup guide (Neovim config, tmux side-pane wrapper, aliases).
    Setup {
        /// Install the bundled `nvim-code` wrapper into DIR (default: ~/.local/bin).
        // `Option<Option<_>>`: outer = flag present, inner = DIR given.
        // (A `default_missing_value = ""` does not work: clap refuses an
        // empty value for the argument.)
        #[arg(long, value_name = "DIR", num_args = 0..=1)]
        install_wrapper: Option<Option<PathBuf>>,

        /// Overwrite an existing, differing wrapper.
        #[arg(long, requires = "install_wrapper")]
        force: bool,
    },
}

/// `promptls setup [--install-wrapper [DIR]] [--force]`.
fn run_setup(install_wrapper: Option<Option<PathBuf>>, force: bool) -> anyhow::Result<()> {
    match install_wrapper {
        Some(dir) => {
            // Flag given without a value: use the default location.
            let dir = match dir {
                Some(dir) => dir,
                None => setup::default_bin_dir()?,
            };
            let dest = setup::install_wrapper(&dir, force)?;
            println!("installed {}", dest.display());
            println!("\nNow add to your shell rc:\n\n{}", setup::ALIASES);
        }
        None => print!("{}", setup::guide()),
    }
    Ok(())
}

/// Directories that count as "temporary": the OS temp dir (honors $TMPDIR)
/// plus the conventional `/tmp`, in case the CLI that spawned us uses a
/// different $TMPDIR than the one we see. Each is listed both as written and
/// canonicalized: on macOS `/tmp` -> `/private/tmp` and `$TMPDIR` lives under
/// `/private/var/folders`, and process cwds come back already resolved.
fn temp_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for raw in [PathBuf::from("/tmp"), std::env::temp_dir()] {
        for d in [raw.clone(), raw.canonicalize().unwrap_or(raw)] {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs
}

fn is_temp(path: &Path) -> bool {
    temp_dirs().iter().any(|t| path.starts_with(t))
}

/// Working directories of this process and its ancestors, nearest first.
/// Claude Code and Codex spawn `$EDITOR` from the project directory, so the
/// first entry is normally already the root; walking further up is a safety
/// net for editor wrappers/multiplexers that start from elsewhere (the CLI
/// process itself always sits in the project). `sysinfo` abstracts the platform
/// (`/proc` on Linux, `proc_pidinfo` on macOS; same-user processes need no
/// elevated privileges on either).
fn ancestor_cwds() -> Vec<(u32, PathBuf)> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut sys = System::new();
    let refresh = ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always);
    let mut out = Vec::new();
    let mut pid = Pid::from_u32(std::process::id());
    // Bounded loop: a pathological process table could otherwise cycle.
    for _ in 0..64 {
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh);
        let Some(proc_) = sys.process(pid) else { break };
        if let Some(cwd) = proc_.cwd() {
            out.push((pid.as_u32(), cwd.to_path_buf()));
        }
        match proc_.parent() {
            Some(ppid) if ppid.as_u32() > 1 && ppid != pid => pid = ppid,
            _ => break,
        }
    }
    out
}

/// First ancestor cwd that is a real directory outside any temp dir.
fn first_non_temp(cwds: &[(u32, PathBuf)]) -> Option<(u32, PathBuf)> {
    cwds.iter().find(|(_, d)| !is_temp(d) && d.is_dir()).cloned()
}

/// Decide the project root.
///
/// Precedence: `--root`, `$PROMPTLS_ROOT`, then the nearest ancestor process
/// whose cwd is outside a temp dir. An explicit root inside a temp dir is an
/// error: the prompt file itself lives there, and indexing /tmp is noise.
fn resolve_root(args: &Args) -> anyhow::Result<PathBuf> {
    let explicit = match &args.root {
        Some(r) => Some(r.clone()),
        None => std::env::var_os("PROMPTLS_ROOT").map(PathBuf::from),
    };
    let root = match explicit {
        Some(candidate) => candidate
            .canonicalize()
            .with_context(|| format!("root {} does not exist", candidate.display()))?,
        None => {
            let cwds = ancestor_cwds();
            let Some((pid, dir)) = first_non_temp(&cwds) else {
                bail!(
                    "no ancestor process has a cwd outside the temp dirs {:?} (saw {:?}); pass --root <project> or set PROMPTLS_ROOT",
                    temp_dirs(),
                    cwds
                );
            };
            tracing::info!("root taken from pid {pid} cwd");
            dir
        }
    };
    if !root.is_dir() {
        bail!("root {} is not a directory", root.display());
    }
    if is_temp(&root) {
        bail!(
            "root {} is inside a temp dir {:?}; pass --root <project> or set PROMPTLS_ROOT",
            root.display(),
            temp_dirs()
        );
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_nearest_non_temp_ancestor() {
        let cwds = vec![(3, PathBuf::from("/tmp")), (2, PathBuf::from("/tmp/x")), (1, PathBuf::from("/usr"))];
        assert_eq!(first_non_temp(&cwds), Some((1, PathBuf::from("/usr"))));
        assert_eq!(first_non_temp(&[(3, PathBuf::from("/tmp"))]), None);
    }

    #[test]
    fn ancestor_walk_includes_self() {
        let cwds = ancestor_cwds();
        assert_eq!(cwds.first().map(|(pid, _)| *pid), Some(std::process::id()));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs go to stderr so they never corrupt the stdio LSP channel.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    if let Some(Command::Setup { install_wrapper, force }) = args.command {
        return run_setup(install_wrapper, force);
    }
    let root = resolve_root(&args)?;
    tracing::info!("promptls root = {}", root.display());

    let (service, socket) = LspService::new(|client| server::Backend::new(client, root, args.max_entries));
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
    Ok(())
}
