#[cfg(not(unix))]
compile_error!("pcron only supports unix");

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use std::{env, path::PathBuf, time::Duration};

mod edit;
mod list;
mod server;

// scripts and the editor always run with sh, whatever the user's $SHELL is
const SHELL: &str = "/bin/sh";

#[derive(Debug, Parser)]
#[command(about, author, version)]
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
}

#[derive(Args, Debug)]
struct DelayArgs {
    /// Lower bound for every sampled delay, in seconds
    #[arg(long, env = "PCRON_MIN_DELAY", default_value_t = 1.0)]
    min_delay: f64,

    /// Upper bound for every sampled delay, in seconds
    #[arg(long, env = "PCRON_MAX_DELAY", default_value_t = 365.0 * 24.0 * 3600.0)]
    max_delay: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let tab = match cli.tab {
        Some(tab) => tab,
        None => default_tab()?,
    };
    match &cli.mode {
        CliMode::Edit => edit::main_edit(&tab).await,
        CliMode::List(args) => list::main_list(&tab, args),
        CliMode::Server(args) => server::main_server(&tab, args).await,
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

#[derive(Clone, Copy, Debug)]
struct DelayBounds {
    min: f64,
    max: f64,
}

impl DelayBounds {
    fn new(min: f64, max: f64) -> Result<Self> {
        ensure!(
            min.is_finite() && min >= 0.0,
            "--min-delay must be a non-negative number, got {min}"
        );
        ensure!(
            max >= min && Duration::try_from_secs_f64(max).is_ok(),
            "--max-delay must be a finite number >= --min-delay, got {max}"
        );
        Ok(Self { min, max })
    }

    fn new_from_args(args: &DelayArgs) -> Result<Self> {
        Self::new(args.min_delay, args.max_delay)
    }

    fn clamp(self, secs: f64) -> f64 {
        // clamp lets NaN through
        if secs.is_nan() {
            self.max
        } else {
            secs.clamp(self.min, self.max)
        }
    }

    fn delay(self, secs: f64) -> Duration {
        Duration::from_secs_f64(self.clamp(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_bounded() {
        let b = DelayBounds::new(1.0, 60.0).unwrap();
        assert_eq!(b.delay(-5.0), Duration::from_secs(1));
        assert_eq!(b.delay(0.0), Duration::from_secs(1));
        assert_eq!(b.delay(30.0), Duration::from_secs(30));
        assert_eq!(b.delay(f64::INFINITY), Duration::from_secs(60));
        assert_eq!(b.delay(f64::NAN), Duration::from_secs(60));
    }

    #[test]
    fn invalid_bounds() {
        assert!(DelayBounds::new(-1.0, 10.0).is_err());
        assert!(DelayBounds::new(10.0, 1.0).is_err());
        assert!(DelayBounds::new(0.0, f64::INFINITY).is_err());
        assert!(DelayBounds::new(0.0, 1e30).is_err());
        assert!(DelayBounds::new(f64::NAN, 1.0).is_err());
    }
}
