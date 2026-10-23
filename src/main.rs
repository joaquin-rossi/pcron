#[cfg(not(unix))]
compile_error!("pcron only supports unix");

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand};
use std::{
    env, fs,
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::OnceLock,
    time::Duration,
};
use tokio::{process::Command, task::JoinSet, time::sleep};

use pcron::*;

mod list;

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
    Edit,
    Server(DelayArgs),
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
        CliMode::Edit => main_edit(&tab).await,
        CliMode::List(args) => list::main_list(&tab, args),
        CliMode::Server(args) => main_server(&tab, args).await,
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

async fn main_edit(tab: &Path) -> Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());
    if let Some(dir) = tab.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot create directory {}", dir.display()))?;
    }

    println!("$ {} {}", editor, tab.display());

    let status = Command::new(SHELL)
        .arg("-c")
        .arg(format!(r#"exec {} "$1""#, editor))
        .arg("sh")
        .arg(tab)
        .status()
        .await
        .with_context(|| format!("cannot launch editor `{editor}`"))?;
    ensure!(status.success(), "editor `{editor}` failed: {status}");

    file::read(tab).context("the tab was saved, but it has errors")?;
    Ok(())
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

async fn main_server(tab_path: &Path, args: &DelayArgs) -> Result<()> {
    let bounds = DelayBounds::new_from_args(args)?;
    let tab = file::read(tab_path)?;
    ensure!(
        !tab.cmds.is_empty(),
        "tab {} has no commands",
        tab_path.display()
    );

    let mut tasks = JoinSet::new();
    for cmd in tab.cmds {
        tasks.spawn(server_handle_cmd(cmd, bounds));
    }

    // command loops never return, so a finished task means it panicked
    match tasks.join_next().await {
        Some(Err(e)) => bail!("a command loop crashed: {e}"),
        _ => unreachable!(),
    }
}

async fn server_handle_cmd(tab_cmd: TabCmd, bounds: DelayBounds) -> ! {
    let mut script_lines = tab_cmd.script.lines();
    let mut log_script = script_lines.next().unwrap_or_default().trim().to_string();
    if script_lines.next().is_some() {
        log_script.push_str(" ; ...");
    }

    loop {
        let sleep_delay = bounds.delay(tab_cmd.dist.sample());
        log_cmd(
            &log_script,
            &format!("SLEEP({:.3}s)", sleep_delay.as_secs_f64()),
        );
        sleep(sleep_delay).await;

        let mut cmd = Command::new(SHELL);
        cmd.arg("-lc");
        cmd.arg(&tab_cmd.script);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        let mut proc = match cmd.spawn() {
            Ok(proc) => proc,
            Err(e) => {
                log_cmd(
                    &log_script,
                    &format!("CMD_ERROR(cannot spawn `{SHELL}`: {e})"),
                );
                continue;
            }
        };

        let proc_pid = proc.id().map(|x| x as i32).unwrap_or(-1);
        log_cmd(&log_script, &format!("CMD_START(pid={})", proc_pid));

        let bg_log_script = log_script.clone();
        tokio::spawn(async move {
            let msg = match proc.wait().await {
                Ok(proc_status) => {
                    format!("CMD_END(pid={}, {})", proc_pid, fmt_status(proc_status))
                }
                Err(e) => format!("CMD_ERROR(pid={}, cannot wait: {e})", proc_pid),
            };
            log_cmd(&bg_log_script, &msg);
        });
    }
}

fn fmt_status(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit={code}"),
        (None, Some(signal)) => format!("signal={signal}"),
        (None, None) => "exit=?".to_string(),
    }
}

pub fn log_cmd(script: &str, msg: &str) {
    let timestamp = chrono::Local::now().format("%b %d %H:%M:%S");

    // neither changes while running, so look them up only once
    static IDENTITY: OnceLock<(String, String)> = OnceLock::new();
    let (user, host) = IDENTITY.get_or_init(|| {
        let user = whoami::username().unwrap_or_else(|_| "<unknown-user>".to_string());
        let host = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "<unknown-host>".to_string());
        (user, host)
    });

    // a closed stdout must not take the server down
    let _ = writeln!(
        io::stdout(),
        "{timestamp} {host}: ({user}) {msg} ({script})"
    );
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
