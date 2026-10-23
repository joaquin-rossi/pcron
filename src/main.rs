#[cfg(not(unix))]
compile_error!("pcron only supports unix");

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use std::{env, io, path::PathBuf};

mod edit;
mod list;
mod man;
mod server;
mod state;

// the editor, and the scripts unless the tab sets SHELL, run with sh (whatever the
// user's $SHELL is)
const SHELL: &str = "/bin/sh";

#[derive(Debug, Parser)]
#[command(about, author, version, after_long_help = include_str!("help.txt"))]
struct Cli {
    /// Path to the tab [default: $XDG_CONFIG_HOME/pcron/tab]
    #[arg(long, env = "PCRON_TAB")]
    tab: Option<PathBuf>,

    #[command(subcommand)]
    mode: CliMode,
}

#[derive(Subcommand, Debug)]
enum CliMode {
    /// Show the commands in the tab with statistics of their delays
    List(list::ListArgs),
    /// Edit a copy of the tab, and install it only if it is valid
    Edit,
    /// Run the commands in the tab (SIGHUP reloads it)
    Server(server::ServerArgs),
    /// Print the completions for a shell
    Completions { shell: clap_complete::Shell },
    /// Print the man page
    Man,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tab = || match &cli.tab {
        Some(tab) => Ok(tab.clone()),
        None => default_tab(),
    };
    match &cli.mode {
        CliMode::Edit => edit::main_edit(&tab()?).await,
        CliMode::List(args) => list::main_list(&tab()?, args),
        CliMode::Server(args) => server::main_server(&tab()?, args).await,
        CliMode::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), "pcron", &mut io::stdout());
            Ok(())
        }
        CliMode::Man => man::render(Cli::command(), &mut io::stdout()),
    }
}

// $XDG_CONFIG_HOME/pcron/tab, falling back to ~/.config/pcron/tab
fn default_tab() -> Result<PathBuf> {
    let absolute = |var| {
        env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let config = absolute("XDG_CONFIG_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".config")))
        .context("cannot locate the tab: neither $XDG_CONFIG_HOME nor $HOME is set, use --tab")?;
    Ok(config.join("pcron").join("tab"))
}
