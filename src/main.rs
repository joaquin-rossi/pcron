#![cfg(unix)]

use chrono::Local;
use clap::{Parser, Subcommand};
use std::{env, path::PathBuf, process::Stdio, time::Duration};
use tokio::{process::Command, time::sleep};
use users::{get_current_uid, get_user_by_uid};

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
    Server,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.mode {
        CliMode::Edit => main_edit(cli).await,
        CliMode::List => main_list(cli),
        CliMode::Server => main_server(cli).await,
    }
}

fn main_list(cli: Cli) {
    let tab_text = std::fs::read_to_string(cli.tab).unwrap();
    print!("{}", tab_text);
}

fn shell_exe() -> String {
    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

async fn main_edit(cli: Cli) {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());
    println!("$ {} {}", editor, cli.tab.display());

    let shell = shell_exe();

    let mut cmd = Command::new(&shell);
    cmd.arg("-lc");
    cmd.arg(format!(r#"exec {} "$1""#, editor));
    cmd.arg(&shell);
    cmd.arg(cli.tab.to_str().unwrap());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    cmd.status().await.unwrap();
}

async fn main_server(cli: Cli) -> ! {
    let tab = parse_file(&cli.tab).unwrap();

    for cmd in tab.cmds {
        tokio::spawn(async move {
            server_handle_cmd(cmd).await;
        });
    }

    futures::future::pending::<()>().await;
    unreachable!()
}

async fn server_handle_cmd(tab_cmd: TabCmd) -> ! {
    let script = format!(r#"exec {}"#, tab_cmd.shell);
    let shell = shell_exe();

    loop {
        let sleep_delay = tab_cmd.dist.sample().max(0.0);
        log_cmd(&tab_cmd.shell, &format!("SLEEP({:.3}s)", sleep_delay));
        sleep(Duration::from_secs_f32(sleep_delay)).await;

        let mut cmd = Command::new(&shell);
        cmd.arg("-lc");
        cmd.arg(&script);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd.status().await.unwrap();
        let mut proc = cmd.spawn().unwrap();

        let proc_pid = proc.id().map(|x| x as i32).unwrap_or(-1);
        log_cmd(&tab_cmd.shell, &format!("CMD_START(pid={})", proc_pid));

        let bg_shell = tab_cmd.shell.clone();
        tokio::spawn(async move {
            let proc_status = proc.wait().await.unwrap();
            log_cmd(
                &bg_shell,
                &format!(
                    "CMD_END(pid={}, exit={})",
                    proc_pid,
                    proc_status.code().unwrap_or(-1)
                ),
            );
        });
    }
}

pub fn log_cmd(cmd: &str, msg: &str) {
    let timestamp = Local::now().format("%b %d %H:%M:%S");

    let user = get_user_by_uid(get_current_uid())
        .and_then(|u| u.name().to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".to_string());

    println!("{timestamp} {hostname}: ({user}) {msg} ({cmd})");
}
