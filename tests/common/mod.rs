//! runs the real binary in a sandbox: a temporary HOME, XDG_STATE_HOME and PCRON_TAB

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

/// generous, so that a slow CI machine doesn't fail them
pub const WAIT: Duration = Duration::from_secs(15);

pub struct Sandbox {
    pub dir: PathBuf,
}

impl Sandbox {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pcron-it-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).unwrap();
        Self { dir }
    }

    pub fn tab(&self) -> PathBuf {
        self.dir.join("tab")
    }

    pub fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pcron"));
        cmd.args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir.join("home"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env("PCRON_TAB", self.tab())
            .env("TZ", "UTC");
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    pub fn server(&self) -> Server {
        self.server_with(&[])
    }

    pub fn server_with(&self, env: &[(&str, &str)]) -> Server {
        let mut child = self
            .cmd(&["server"])
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = mpsc::channel();
        for out in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            });
        }
        let mut server = Server {
            child,
            lines: rx,
            log: Vec::new(),
        };
        server.wait_for("START(");
        server
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub struct Server {
    child: Child,
    lines: Receiver<String>,
    /// every line so far
    pub log: Vec<String>,
}

impl Server {
    /// the first line from now on that contains `text`
    pub fn wait_for(&mut self, text: &str) -> String {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    self.log.push(line.clone());
                    if line.contains(text) {
                        return line;
                    }
                }
                Err(_) => panic!("no `{text}` in the log:\n{}", self.log.join("\n")),
            }
        }
    }

    pub fn signal(&self, signal: libc::c_int) {
        // SAFETY: kill has no memory safety requirements
        unsafe { libc::kill(self.child.id() as libc::pid_t, signal) };
    }

    /// SIGTERM, and the rest of the log until it exits
    pub fn stop(mut self) -> (bool, Vec<String>) {
        self.signal(libc::SIGTERM);
        let status = wait_child(&mut self.child);
        // the readers end when the pipes close
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(1)) {
            self.log.push(line);
        }
        (status, std::mem::take(&mut self.log))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_child(child: &mut Child) -> bool {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.success();
        }
        assert!(Instant::now() < deadline, "the server did not exit");
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn exists(path: &Path) -> bool {
    path.exists()
}
