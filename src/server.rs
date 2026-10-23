use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, Utc};
use clap::{Args, ValueEnum};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{self, Write},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::{Child, Command},
    signal::unix::{Signal, SignalKind, signal},
    sync::{Notify, watch},
    task::{AbortHandle, JoinSet},
    time::{Instant, sleep, sleep_until, timeout},
};

use crate::{
    SHELL,
    state::{self, CmdKey, Saved, Timers},
};
use crate::{Settings, Tab, TabCmd, file, schedule};

// longer lines of script output are cut
const MAX_LINE: usize = 4096;
// the last lines of output that PCRON_ON_FAILURE gets
const TAIL_LINES: usize = 20;

#[derive(Args, Debug)]
pub struct ServerArgs {
    /// What to do with the stdout and stderr of the scripts
    #[arg(long, value_enum, env = "PCRON_OUTPUT", default_value_t = Output::Log)]
    output: Output,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Output {
    /// Log every line, as CMD_OUT / CMD_ERR
    Log,
    /// Throw it away
    Discard,
}

struct Ctx {
    output: Output,
    children: Children,
    /// what the running scripts should do; each one stops (or is killed) on its own
    phase: watch::Sender<Phase>,
    timers: Arc<Timers>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Running,
    /// SIGTERM now, SIGKILL when its time is up
    Stopping,
    /// SIGKILL now
    Killing,
}

pub async fn main_server(tab_path: &Path, args: &ServerArgs) -> Result<()> {
    let tab = read_tab(tab_path)?;

    let listen = |kind, name| signal(kind).with_context(|| format!("cannot listen for {name}"));
    let mut hup = listen(SignalKind::hangup(), "SIGHUP")?;
    let mut term = listen(SignalKind::terminate(), "SIGTERM")?;
    let mut int = listen(SignalKind::interrupt(), "SIGINT")?;

    let state_path = state::path(tab_path);
    let mut saved = match &state_path {
        Some(path) => state::load(path).unwrap_or_else(|e| {
            log_line(&format!("TIMERS_ERROR({e:#}; starting over)"));
            Saved::new()
        }),
        None => {
            log_line(
                "TIMERS_ERROR(none of $PCRON_STATE, $XDG_STATE_HOME or $HOME is set; \
                 not saving them)",
            );
            Saved::new()
        }
    };

    let ctx = Arc::new(Ctx {
        output: args.output,
        children: Children::default(),
        phase: watch::Sender::new(Phase::Running),
        timers: Arc::default(),
    });
    let mut loops = Loops::default();
    log_line(&format!("START(commands={})", tab.cmds.len()));
    loops.sync(tab.cmds, &ctx, &mut saved);

    let saver = state_path.clone().map(|path| {
        tokio::spawn(save_timers(
            ctx.timers.clone(),
            path,
            tab_path.to_path_buf(),
        ))
    });

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
                    let s = loops.sync(tab.cmds, &ctx, &mut Saved::new());
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

    // save the timers as they are now: stopping the loops forgets them
    if let Some(saver) = saver {
        saver.abort();
    }
    if let Some(path) = &state_path {
        if let Err(e) = ctx.timers.save(path, tab_path) {
            log_line(&format!("TIMERS_ERROR({e:#})"));
        }
    }
    loops.set.abort_all();

    let reason = match &result {
        Ok(signal) => signal,
        Err(_) => "crash",
    };
    shutdown(&ctx, reason, &mut term, &mut int).await;
    result.map(|_| ())
}

// at most once a second, however often they change
async fn save_timers(timers: Arc<Timers>, path: PathBuf, tab: PathBuf) {
    loop {
        timers.changed.notified().await;
        sleep(Duration::from_secs(1)).await;
        if let Err(e) = timers.save(&path, &tab) {
            log_line(&format!("TIMERS_ERROR({e:#})"));
        }
    }
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

// with the scheduling already stopped: ask the running scripts to finish (each one is
// killed when its time is up, see watch_child), or kill them all if another SIGTERM /
// SIGINT arrives
async fn shutdown(ctx: &Ctx, reason: &str, term: &mut Signal, int: &mut Signal) {
    let running = ctx.children.len();
    log_line(&format!("SHUTDOWN({reason}, running={running})"));

    if running > 0 {
        ctx.phase.send_replace(Phase::Stopping);
        let finished = tokio::select! {
            () = ctx.children.wait_empty() => true,
            _ = term.recv() => false,
            _ = int.recv() => false,
        };
        if !finished {
            log_line(&format!("SHUTDOWN(killing={})", ctx.children.len()));
            ctx.phase.send_replace(Phase::Killing);
            // give them a moment to be reaped, so that their CMD_END gets logged
            let _ = timeout(Duration::from_secs(1), ctx.children.wait_empty()).await;
        }
    }

    log_line("SHUTDOWN(done)");
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
    // repeated commands are matched by count; new ones take their timer from `saved` if
    // it has one
    fn sync(&mut self, cmds: Vec<TabCmd>, ctx: &Arc<Ctx>, saved: &mut Saved) -> SyncStats {
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
            let handles = self.by_key.entry(key.clone()).or_default();
            let have = handles.len();
            for cmd in cmds.into_iter().skip(have) {
                let resume = saved.get_mut(&key).and_then(Vec::pop);
                let run = server_handle_cmd(cmd, key.clone(), resume, ctx.clone());
                handles.push(self.set.spawn(run));
                stats.started += 1;
            }
        }

        stats
    }
}

/// what a command runs: its script, and its PCRON_ON_FAILURE when that fails
struct Job {
    /// for the logs: the name, or else the start of the script
    label: String,
    name: Option<String>,
    script: String,
    shell: String,
    /// without the PCRON_ settings
    env: Vec<(String, String)>,
    settings: Settings,
}

impl Job {
    fn new(cmd: &TabCmd) -> Self {
        let label = cmd.name.clone().unwrap_or_else(|| {
            let mut lines = cmd.script.lines();
            let mut start = lines.next().unwrap_or_default().trim().to_string();
            if lines.next().is_some() {
                start.push_str(" ; ...");
            }
            start
        });
        Self {
            label,
            name: cmd.name.clone(),
            script: cmd.script.clone(),
            shell: script_shell(&cmd.env).to_string(),
            env: cmd
                .env
                .iter()
                .filter(|(k, _)| !file::SETTINGS.contains(&k.as_str()))
                .cloned()
                .collect(),
            settings: cmd.settings.clone(),
        }
    }
}

async fn server_handle_cmd(
    cmd: TabCmd,
    key: CmdKey,
    mut resume: Option<DateTime<Utc>>,
    ctx: Arc<Ctx>,
) {
    let job = Arc::new(Job::new(&cmd));
    let timer = ctx.timers.register(key);
    let mut from = Utc::now();

    loop {
        let until = match resume.take() {
            Some(until) if until <= Utc::now() => {
                let missed = until
                    .with_timezone(&chrono::Local)
                    .format("%b %d %Y %H:%M:%S");
                log_cmd(&job.label, &format!("RESUME(missed {missed}, running now)"));
                until
            }
            Some(until) => {
                log_cmd(&job.label, &format!("RESUME(until {})", fmt_time(until)));
                until
            }
            None => {
                let (secs, until) = schedule::next_run(&cmd, from, &mut rand::rng());
                let window = cmd
                    .window
                    .as_ref()
                    .map_or(String::new(), |w| format!(" in @ {w}"));
                log_cmd(
                    &job.label,
                    &format!("SLEEP({secs:.3}s{window}, until {})", fmt_time(until)),
                );
                until
            }
        };
        timer.set(until);
        from = wait_until(&cmd, until).await;
        start(&job, None, &ctx);
    }
}

fn fmt_time(t: DateTime<Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%b %d %H:%M:%S")
        .to_string()
}

// in short steps, so that a suspend or a clock change can't throw it off; with a window,
// until it is open too (say, after waking up from a suspend at night); returns when
async fn wait_until(cmd: &TabCmd, mut until: DateTime<Utc>) -> DateTime<Utc> {
    const STEP: Duration = Duration::from_secs(60);
    loop {
        let now = Utc::now();
        if now >= until {
            let open = schedule::open_at(cmd, now);
            if open <= now {
                return now;
            }
            until = open;
            continue;
        }
        let left = (until - now).to_std().unwrap_or_default();
        sleep(left.min(STEP)).await;
    }
}

// like cron: SHELL from the tab, or /bin/sh
fn script_shell(env: &[(String, String)]) -> &str {
    env.iter()
        .find(|(k, v)| k == "SHELL" && !v.is_empty())
        .map_or(SHELL, |(_, v)| v)
}

/// why a script failed, for its PCRON_ON_FAILURE
struct Failure {
    /// exit, signal, timeout or spawn
    reason: &'static str,
    exit: Option<i32>,
    signal: Option<i32>,
    output: String,
}

// the script of `job`, or its PCRON_ON_FAILURE if there is a failure
fn start(job: &Arc<Job>, failure: Option<Failure>, ctx: &Arc<Ctx>) {
    let (text, prefix) = match &failure {
        None => (job.script.as_str(), "CMD"),
        Some(_) => match &job.settings.on_failure {
            Some(hook) => (hook.as_str(), "HOOK"),
            None => return,
        },
    };

    let mut cmd = Command::new(&job.shell);
    cmd.arg("-c");
    cmd.arg(text);
    cmd.envs(job.env.iter().map(|(k, v)| (k, v)));
    cmd.env("SHELL", &job.shell);
    if let Some(f) = &failure {
        let number = |n: Option<i32>| n.map(|n| n.to_string()).unwrap_or_default();
        cmd.env("PCRON_NAME", job.name.as_deref().unwrap_or_default());
        cmd.env("PCRON_SCRIPT", &job.script);
        cmd.env("PCRON_REASON", f.reason);
        cmd.env("PCRON_EXIT", number(f.exit));
        cmd.env("PCRON_SIGNAL", number(f.signal));
        cmd.env("PCRON_OUTPUT", &f.output);
    }
    cmd.stdin(Stdio::null());
    match ctx.output {
        Output::Log => cmd.stdout(Stdio::piped()).stderr(Stdio::piped()),
        Output::Discard => cmd.stdout(Stdio::null()).stderr(Stdio::null()),
    };
    // own process group, so that signals reach whatever the script started too
    cmd.process_group(0);

    let spawned = cmd.spawn().map_err(|e| e.to_string()).and_then(|proc| {
        let pid = proc.id().ok_or("it exited before it got a pid")?;
        Ok((proc, pid))
    });
    let (proc, pid) = match spawned {
        Ok(spawned) => spawned,
        Err(e) => {
            log_cmd(
                &job.label,
                &format!("{prefix}_ERROR(cannot spawn `{}`: {e})", job.shell),
            );
            if failure.is_none() {
                let failure = Failure {
                    reason: "spawn",
                    exit: None,
                    signal: None,
                    output: e,
                };
                on_failure(job, failure, ctx);
            }
            return;
        }
    };
    // registered before any await, so that an abort cannot leave it untracked
    ctx.children.insert(pid);
    log_cmd(&job.label, &format!("{prefix}_START(pid={pid})"));

    // not part of the loop task: a reload that stops the loop lets the script finish
    let hook = failure.is_some();
    tokio::spawn(watch_child(proc, pid, job.clone(), hook, ctx.clone()));
}

// not while shutting down: the scripts it stops are not failures
fn on_failure(job: &Arc<Job>, failure: Failure, ctx: &Arc<Ctx>) {
    if *ctx.phase.borrow() == Phase::Running {
        start(job, Some(failure), ctx);
    }
}

// logs its output and its end, and stops it: SIGTERM after PCRON_TIMEOUT and SIGKILL
// PCRON_SHUTDOWN_TIMEOUT later; on shutdown, SIGTERM right away and SIGKILL after the
// shorter of PCRON_SHUTDOWN_TIMEOUT and what is left of its PCRON_TIMEOUT. A failed script
// (but not a failed hook) runs its PCRON_ON_FAILURE.
async fn watch_child(mut proc: Child, pid: u32, job: Arc<Job>, hook: bool, ctx: Arc<Ctx>) {
    let prefix = if hook { "HOOK" } else { "CMD" };
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let mut readers = JoinSet::new();
    if let Some(out) = proc.stdout.take() {
        let kind = format!("{prefix}_OUT");
        readers.spawn(forward_lines(
            out,
            kind,
            pid,
            job.label.clone(),
            tail.clone(),
        ));
    }
    if let Some(err) = proc.stderr.take() {
        let kind = format!("{prefix}_ERR");
        readers.spawn(forward_lines(
            err,
            kind,
            pid,
            job.label.clone(),
            tail.clone(),
        ));
    }

    let settings = &job.settings;
    let timeout_at = settings.timeout.map(|t| Instant::now() + t);
    let mut kill_at: Option<Instant> = None;
    let mut asked = false;
    let mut timed_out = false;
    let mut phase = ctx.phase.subscribe();
    // it may have started while already shutting down
    phase.mark_changed();

    let status = loop {
        tokio::select! {
            status = proc.wait() => break status,
            () = sleep_until_some(timeout_at), if !asked => {
                log_cmd(&job.label, &format!("{prefix}_TIMEOUT(pid={pid})"));
                signal_group(pid, libc::SIGTERM);
                asked = true;
                timed_out = true;
                kill_at = earliest(kill_at, Some(Instant::now() + settings.shutdown_timeout));
            }
            () = sleep_until_some(kill_at) => {
                log_cmd(&job.label, &format!("{prefix}_KILL(pid={pid})"));
                signal_group(pid, libc::SIGKILL);
                kill_at = None;
            }
            Ok(()) = phase.changed() => match *phase.borrow_and_update() {
                Phase::Running => {}
                Phase::Stopping => {
                    if !asked {
                        signal_group(pid, libc::SIGTERM);
                        asked = true;
                    }
                    let grace = Instant::now() + settings.shutdown_timeout;
                    kill_at = earliest(kill_at, earliest(Some(grace), timeout_at));
                }
                Phase::Killing => kill_at = Some(Instant::now()),
            },
        }
    };

    // let the readers drain what is left; something the script left in the background
    // may keep the pipes open, so don't wait for it forever
    let _ = timeout(Duration::from_secs(1), readers.join_all()).await;
    let failure = match &status {
        Ok(status) => {
            log_cmd(
                &job.label,
                &format!("{prefix}_END(pid={pid}, {})", fmt_status(*status)),
            );
            let reason = if timed_out {
                Some("timeout")
            } else if status.success() {
                None
            } else if status.code().is_some() {
                Some("exit")
            } else {
                Some("signal")
            };
            reason.map(|reason| Failure {
                reason,
                exit: status.code(),
                signal: status.signal(),
                output: Vec::from(tail.lock().unwrap().clone()).join("\n"),
            })
        }
        Err(e) => {
            log_cmd(
                &job.label,
                &format!("{prefix}_ERROR(pid={pid}, cannot wait: {e})"),
            );
            None
        }
    };

    ctx.children.remove(pid);
    if let (Some(failure), false) = (failure, hook) {
        on_failure(&job, failure, &ctx);
    }
}

async fn forward_lines(
    r: impl AsyncRead + Unpin,
    kind: String,
    pid: u32,
    label: String,
    tail: Arc<Mutex<VecDeque<String>>>,
) {
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
        log_cmd(&label, &format!("{kind}(pid={pid}) {text}{mark}"));

        let mut tail = tail.lock().unwrap();
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(format!("{text}{mark}"));
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

async fn sleep_until_some(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        _ => a.or(b),
    }
}

// the pid of a script is also its process group, so this reaches whatever it started too
fn signal_group(pid: u32, signal: libc::c_int) {
    if let Ok(pgid) = libc::pid_t::try_from(pid) {
        // SAFETY: kill has no memory safety requirements; a negative pid targets the group
        unsafe { libc::kill(-pgid, signal) };
    }
}

/// running scripts, by pid
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

    #[test]
    fn shell() {
        let shell = |tab| script_shell(&cmds(tab)[0].env).to_string();
        assert_eq!(shell("1 $ a"), "/bin/sh");
        assert_eq!(shell("SHELL=/bin/bash\n1 $ a"), "/bin/bash");
        assert_eq!(shell("SHELL=\n1 $ a"), "/bin/sh");
    }

    #[tokio::test]
    async fn reload_matches_commands_by_content() {
        let ctx = Arc::new(Ctx {
            output: Output::Discard,
            children: Children::default(),
            phase: watch::Sender::new(Phase::Running),
            timers: Arc::default(),
        });
        let sync = |loops: &mut Loops, tab| loops.sync(cmds(tab), &ctx, &mut Saved::new());
        let mut loops = Loops::default();

        let s = sync(&mut loops, "1 $ a\n2 $ b\n1 $ a");
        assert_eq!((s.kept, s.started, s.stopped), (0, 3, 0));

        // reordered and rewritten (2 == 4/2) commands keep their loop; one `a` goes
        // away, `c` is new, and `a` with another distribution is a different command
        let s = sync(&mut loops, "4/2 $ b\n( 1 ) $ a\n1 $ c\n3 $ a");
        assert_eq!((s.kept, s.started, s.stopped), (2, 2, 1));
        assert_eq!(loops.by_key.values().map(Vec::len).sum::<usize>(), 4);

        let s = sync(&mut loops, "1 $ c");
        assert_eq!((s.kept, s.started, s.stopped), (1, 0, 3));

        // the same command with another environment is a different command
        let s = sync(&mut loops, "X=1\n1 $ c");
        assert_eq!((s.kept, s.started, s.stopped), (0, 1, 1));

        // and so are a name or a window
        let s = sync(&mut loops, "X=1\nc: 1 $ c\nX=1\n1 @ 9-18 $ c");
        assert_eq!((s.kept, s.started, s.stopped), (0, 2, 1));
    }
}
