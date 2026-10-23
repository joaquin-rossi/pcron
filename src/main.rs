#![cfg(unix)]

use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use std::{env, path::PathBuf, process::Stdio, time::Duration};
use tokio::{process::Command, time::sleep};

use pcron::*;

#[derive(Debug, Parser)]
#[command(about, author, version)]
struct Cli {
    #[arg(long, env = "PCRON_TAB", default_value = "./tab")]
    tab: PathBuf,

    #[command(subcommand)]
    mode: CliMode,
}

#[derive(Subcommand, Debug)]
enum CliMode {
    List,
    Edit,
    Server(ServerArgs),
}

#[derive(Args, Debug)]
struct ServerArgs {
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
    match &cli.mode {
        CliMode::Edit => main_edit(&cli).await,
        CliMode::List => main_list(&cli),
        CliMode::Server(args) => main_server(&cli, args).await,
    }
}

fn main_list(cli: &Cli) -> Result<()> {
    let tab_text = std::fs::read_to_string(&cli.tab)
        .with_context(|| format!("cannot read tab {}", cli.tab.display()))?;
    print!("{}", tab_text);
    Ok(())
}

fn shell_exe() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

async fn main_edit(cli: &Cli) -> Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());
    println!("$ {} {}", editor, cli.tab.display());

    // /bin/sh and not $SHELL: the snippet below is POSIX sh syntax
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(r#"exec {} "$1""#, editor))
        .arg("sh")
        .arg(&cli.tab)
        .status()
        .await
        .with_context(|| format!("cannot launch editor `{editor}`"))?;
    ensure!(status.success(), "editor `{editor}` failed: {status}");

    file::read(&cli.tab)?;
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

    fn delay(self, sample: f32) -> Duration {
        let secs = f64::from(sample);
        // clamp lets NaN through
        let secs = if secs.is_nan() {
            self.max
        } else {
            secs.clamp(self.min, self.max)
        };
        Duration::from_secs_f64(secs)
    }
}

async fn main_server(cli: &Cli, args: &ServerArgs) -> Result<()> {
    let bounds = DelayBounds::new(args.min_delay, args.max_delay)?;
    let tab = file::read(&cli.tab)?;

    for cmd in tab.cmds {
        tokio::spawn(async move {
            server_handle_cmd(cmd, bounds).await;
        });
    }

    futures::future::pending::<()>().await;
    unreachable!()
}

async fn server_handle_cmd(tab_cmd: TabCmd, bounds: DelayBounds) -> ! {
    let shell = shell_exe();

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

        let mut cmd = Command::new(&shell);
        cmd.arg("-lc");
        cmd.arg(&tab_cmd.script);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        let mut proc = match cmd.spawn() {
            Ok(proc) => proc,
            Err(e) => {
                log_cmd(&log_script, &format!("CMD_ERROR({e})"));
                continue;
            }
        };

        let proc_pid = proc.id().map(|x| x as i32).unwrap_or(-1);
        log_cmd(&log_script, &format!("CMD_START(pid={})", proc_pid));

        let bg_log_script = log_script.clone();
        tokio::spawn(async move {
            let msg = match proc.wait().await {
                Ok(proc_status) => format!(
                    "CMD_END(pid={}, exit={})",
                    proc_pid,
                    proc_status.code().unwrap_or(-1)
                ),
                Err(e) => format!("CMD_ERROR(pid={}, {e})", proc_pid),
            };
            log_cmd(&bg_log_script, &msg);
        });
    }
}

pub fn log_cmd(script: &str, msg: &str) {
    let timestamp = chrono::Local::now().format("%b %d %H:%M:%S");

    let user = whoami::username().unwrap_or_else(|_| "<unknown-user>".to_string());

    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "<unknown-host>".to_string());

    println!("{timestamp} {host}: ({user}) {msg} ({script})");
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
        assert_eq!(b.delay(f32::INFINITY), Duration::from_secs(60));
        assert_eq!(b.delay(f32::NAN), Duration::from_secs(60));
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
