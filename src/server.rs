use anyhow::{Context, Result, anyhow, ensure};
use clap::{Args, ValueEnum};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::{Child, Command},
    signal::unix::{Signal, SignalKind, signal},
    sync::Notify,
    task::{AbortHandle, JoinSet},
    time::{sleep, timeout},
};

use crate::{DelayArgs, DelayBounds, SHELL};
use pcron::{Tab, TabCmd, file};

// longer lines of script output are cut
const MAX_LINE: usize = 4096;

#[derive(Args, Debug)]
pub struct ServerArgs {
    #[command(flatten)]
    delay: DelayArgs,

    /// What to do with the stdout and stderr of the scripts
    #[arg(long, value_enum, env = "PCRON_OUTPUT", default_value_t = Output::Log)]
    output: Output,

    /// Seconds to wait for running scripts on shutdown before killing them
    #[arg(long, env = "PCRON_SHUTDOWN_TIMEOUT", default_value_t = 10.0)]
    shutdown_timeout: f64,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Output {
    /// Log every line, as CMD_OUT / CMD_ERR
    Log,
    /// Throw it away
    Discard,
}

struct Ctx {
    bounds: DelayBounds,
    output: Output,
    children: Children,
}

pub async fn main_server(tab_path: &Path, args: &ServerArgs) -> Result<()> {
    let bounds = DelayBounds::new_from_args(&args.delay)?;
    let grace = Duration::try_from_secs_f64(args.shutdown_timeout).map_err(|_| {
        anyhow!(
            "--shutdown-timeout must be a non-negative number of seconds, got {}",
            args.shutdown_timeout
        )
    })?;
    let tab = read_tab(tab_path)?;

    let listen = |kind, name| signal(kind).with_context(|| format!("cannot listen for {name}"));
    let mut hup = listen(SignalKind::hangup(), "SIGHUP")?;
    let mut term = listen(SignalKind::terminate(), "SIGTERM")?;
    let mut int = listen(SignalKind::interrupt(), "SIGINT")?;

    let ctx = Arc::new(Ctx {
        bounds,
        output: args.output,
        children: Children::default(),
    });
    let mut loops = Loops::default();
    let n = tab.cmds.len();
    loops.sync(tab.cmds, &ctx);
    log_line(&format!("START(commands={n})"));

    let result = loop {
        tokio::select! {
            Some(res) = loops.set.join_next() => match res {
                Err(e) if e.is_cancelled() => {}
                // command loops never return, so anything else means a panic
                Err(e) => break Err(anyhow!("a command loop crashed: {e}")),
                Ok(()) => break Err(anyhow!("a command loop ended unexpectedly")),
            },
            _ = hup.recv() => match read_tab(tab_path) {
                Ok(tab) => {
                    let s = loops.sync(tab.cmds, &ctx);
                    log_line(&format!(
                        "RELOAD(kept={}, started={}, stopped={})",
                        s.kept, s.started, s.stopped
                    ));
                }
                Err(e) => log_line(&format!("RELOAD_ERROR({e:#}; keeping the previous tab)")),
            },
            _ = term.recv() => break Ok("SIGTERM"),
            _ = int.recv() => break Ok("SIGINT"),
        }
    };

    loops.set.abort_all();
    let reason = match &result {
        Ok(signal) => signal,
        Err(_) => "crash",
    };
    shutdown(&ctx, grace, reason, &mut term, &mut int).await;
    result.map(|_| ())
}

fn read_tab(tab_path: &Path) -> Result<Tab> {
    let tab = file::read(tab_path)?;
    ensure!(
        !tab.cmds.is_empty(),
        "tab {} has no commands",
        tab_path.display()
    );
    Ok(tab)
}

// stop scheduling, ask the running scripts to finish, and kill them if they take too long
// (or if another SIGTERM / SIGINT arrives)
async fn shutdown(ctx: &Ctx, grace: Duration, reason: &str, term: &mut Signal, int: &mut Signal) {
    let running = ctx.children.len();
    log_line(&format!("SHUTDOWN({reason}, running={running})"));

    if running > 0 {
        ctx.children.signal_all(libc::SIGTERM);
        let finished = tokio::select! {
            res = timeout(grace, ctx.children.wait_empty()) => res.is_ok(),
            _ = term.recv() => false,
            _ = int.recv() => false,
        };
        if !finished {
            log_line(&format!("SHUTDOWN(killing={})", ctx.children.len()));
            ctx.children.signal_all(libc::SIGKILL);
            // give them a moment to be reaped, so that their CMD_END gets logged
            let _ = timeout(Duration::from_secs(1), ctx.children.wait_empty()).await;
        }
    }

    log_line("SHUTDOWN(done)");
}

/// identifies a command by its content, so that a reload can tell which ones did not change
#[derive(Debug, PartialEq, Eq, Hash)]
struct CmdKey {
    name: String,
    args: Vec<u64>,
    script: String,
}

impl CmdKey {
    fn new(cmd: &TabCmd) -> Self {
        Self {
            name: cmd.name.clone(),
            // `+ 0.0` turns -0 into 0, so that both get the same bits
            args: cmd.args.iter().map(|a| (a + 0.0).to_bits()).collect(),
            script: cmd.script.clone(),
        }
    }
}

#[derive(Default)]
struct Loops {
    set: JoinSet<()>,
    by_key: HashMap<CmdKey, Vec<AbortHandle>>,
}

#[derive(Debug, Default, PartialEq)]
struct SyncStats {
    kept: usize,
    started: usize,
    stopped: usize,
}

impl Loops {
    // make the running loops match `cmds`: unchanged commands keep their loop (and timer),
    // repeated commands are matched by count
    fn sync(&mut self, cmds: Vec<TabCmd>, ctx: &Arc<Ctx>) -> SyncStats {
        let mut wanted: HashMap<CmdKey, Vec<TabCmd>> = HashMap::new();
        for cmd in cmds {
            wanted.entry(CmdKey::new(&cmd)).or_default().push(cmd);
        }

        let mut stats = SyncStats::default();
        self.by_key.retain(|key, handles| {
            let n = wanted.get(key).map_or(0, Vec::len);
            while handles.len() > n {
                handles.pop().expect("len > n >= 0").abort();
                stats.stopped += 1;
            }
            stats.kept += handles.len();
            !handles.is_empty()
        });

        for (key, cmds) in wanted {
            let handles = self.by_key.entry(key).or_default();
            let have = handles.len();
            for cmd in cmds.into_iter().skip(have) {
                handles.push(self.set.spawn(server_handle_cmd(cmd, ctx.clone())));
                stats.started += 1;
            }
        }

        stats
    }
}

async fn server_handle_cmd(tab_cmd: TabCmd, ctx: Arc<Ctx>) {
    let mut script_lines = tab_cmd.script.lines();
    let mut log_script = script_lines.next().unwrap_or_default().trim().to_string();
    if script_lines.next().is_some() {
        log_script.push_str(" ; ...");
    }

    loop {
        let sleep_delay = ctx.bounds.delay(tab_cmd.dist.sample());
        log_cmd(
            &log_script,
            &format!("SLEEP({:.3}s)", sleep_delay.as_secs_f64()),
        );
        sleep(sleep_delay).await;

        let mut cmd = Command::new(SHELL);
        cmd.arg("-lc");
        cmd.arg(&tab_cmd.script);
        cmd.stdin(Stdio::null());
        match ctx.output {
            Output::Log => cmd.stdout(Stdio::piped()).stderr(Stdio::piped()),
            Output::Discard => cmd.stdout(Stdio::null()).stderr(Stdio::null()),
        };
        // own process group, so that signals reach whatever the script started too
        cmd.process_group(0);

        let proc = match cmd.spawn() {
            Ok(proc) => proc,
            Err(e) => {
                log_cmd(
                    &log_script,
                    &format!("CMD_ERROR(cannot spawn `{SHELL}`: {e})"),
                );
                continue;
            }
        };
        let Some(pid) = proc.id() else {
            log_cmd(
                &log_script,
                "CMD_ERROR(the script exited before it got a pid)",
            );
            continue;
        };
        // registered before any await, so that an abort cannot leave it untracked
        ctx.children.insert(pid);
        log_cmd(&log_script, &format!("CMD_START(pid={pid})"));

        // not part of the loop task: a reload that stops the loop lets the script finish
        tokio::spawn(watch_child(proc, pid, log_script.clone(), ctx.clone()));
    }
}

async fn watch_child(mut proc: Child, pid: u32, log_script: String, ctx: Arc<Ctx>) {
    let mut readers = JoinSet::new();
    if let Some(out) = proc.stdout.take() {
        readers.spawn(forward_lines(out, "CMD_OUT", pid, log_script.clone()));
    }
    if let Some(err) = proc.stderr.take() {
        readers.spawn(forward_lines(err, "CMD_ERR", pid, log_script.clone()));
    }

    let msg = match proc.wait().await {
        Ok(status) => format!("CMD_END(pid={pid}, {})", fmt_status(status)),
        Err(e) => format!("CMD_ERROR(pid={pid}, cannot wait: {e})"),
    };
    // let the readers drain what is left; something the script left in the background
    // may keep the pipes open, so don't wait for it forever
    let _ = timeout(Duration::from_secs(1), readers.join_all()).await;
    log_cmd(&log_script, &msg);

    ctx.children.remove(pid);
}

async fn forward_lines(r: impl AsyncRead + Unpin, kind: &str, pid: u32, log_script: String) {
    let mut r = BufReader::new(r);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut r)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }

        let cut = line.last() != Some(&b'\n') && line.len() == MAX_LINE;
        if cut && skip_line(&mut r).await.is_err() {
            return;
        }

        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\n', '\r']);
        let mark = if cut { " …" } else { "" };
        log_cmd(&log_script, &format!("{kind}(pid={pid}) {text}{mark}"));
    }
}

// consume up to the end of the current line
async fn skip_line(r: &mut (impl AsyncBufRead + Unpin)) -> io::Result<()> {
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(());
        }
        let (n, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        r.consume(n);
        if done {
            return Ok(());
        }
    }
}

/// running scripts, by pid (which is also their process group)
#[derive(Default)]
struct Children {
    pids: Mutex<HashSet<u32>>,
    exited: Notify,
}

impl Children {
    fn insert(&self, pid: u32) {
        self.pids.lock().unwrap().insert(pid);
    }

    fn remove(&self, pid: u32) {
        self.pids.lock().unwrap().remove(&pid);
        self.exited.notify_waiters();
    }

    fn len(&self) -> usize {
        self.pids.lock().unwrap().len()
    }

    fn signal_all(&self, signal: libc::c_int) {
        for &pid in self.pids.lock().unwrap().iter() {
            let Ok(pgid) = libc::pid_t::try_from(pid) else {
                continue;
            };
            // SAFETY: kill has no memory safety requirements; a negative pid targets the group
            unsafe { libc::kill(-pgid, signal) };
        }
    }

    async fn wait_empty(&self) {
        loop {
            // created before checking, so a removal in between is not missed
            let exited = self.exited.notified();
            if self.len() == 0 {
                return;
            }
            exited.await;
        }
    }
}

fn fmt_status(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit={code}"),
        (None, Some(signal)) => format!("signal={signal}"),
        (None, None) => "exit=?".to_string(),
    }
}

fn log_cmd(script: &str, msg: &str) {
    log_line(&format!("{msg} ({script})"));
}

fn log_line(msg: &str) {
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
    let _ = writeln!(io::stdout(), "{timestamp} {host}: ({user}) {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmds(tab: &str) -> Vec<TabCmd> {
        file::parse(tab).unwrap().cmds
    }

    #[tokio::test]
    async fn reload_matches_commands_by_content() {
        let ctx = Arc::new(Ctx {
            bounds: DelayBounds::new(1000.0, 1000.0).unwrap(),
            output: Output::Discard,
            children: Children::default(),
        });
        let mut loops = Loops::default();

        let s = loops.sync(cmds("const(1) $ a\nconst(2) $ b\nconst(1) $ a"), &ctx);
        assert_eq!((s.kept, s.started, s.stopped), (0, 3, 0));

        // reordered and rewritten (2 == 4/2) commands keep their loop; one `a` goes
        // away, `c` is new, and `a` with another distribution is a different command
        let s = loops.sync(
            cmds("const(4/2) $ b\nconst( 1 ) $ a\nconst(1) $ c\nconst(3) $ a"),
            &ctx,
        );
        assert_eq!((s.kept, s.started, s.stopped), (2, 2, 1));
        assert_eq!(loops.by_key.values().map(Vec::len).sum::<usize>(), 4);

        let s = loops.sync(cmds("const(1) $ c"), &ctx);
        assert_eq!((s.kept, s.started, s.stopped), (1, 0, 3));
    }
}
