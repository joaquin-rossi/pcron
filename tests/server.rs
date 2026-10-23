mod common;

use common::Sandbox;
use std::time::{Duration, Instant};

// runs right away (delays this short need PCRON_MIN_DELAY=0)
const FAST: &str = "PCRON_MIN_DELAY=0\n";

#[test]
fn runs_scripts_and_logs_their_output() {
    let sb = Sandbox::new("output");
    sb.write(
        "tab",
        &format!("{FAST}GREETING=hi\nhello: 0.1 $ echo \"$GREETING\"; echo oops >&2; exit 3\n"),
    );
    let mut server = sb.server();
    assert!(server.wait_for("CMD_OUT").ends_with("hi (hello)"));
    assert!(server.wait_for("CMD_ERR").ends_with("oops (hello)"));
    assert!(server.wait_for("CMD_END").contains("exit=3"));

    let (ok, log) = server.stop();
    assert!(ok, "{log:?}");
    assert!(log.last().unwrap().contains("SHUTDOWN(done)"));
}

#[test]
fn timeout_and_on_failure() {
    let sb = Sandbox::new("timeout");
    sb.write(
        "tab",
        &format!(
            "{FAST}PCRON_TIMEOUT=0.3\nPCRON_SHUTDOWN_TIMEOUT=0.3\n\
             PCRON_ON_FAILURE=echo \"failed: $PCRON_NAME $PCRON_REASON\"\n\
             stubborn: 0.1 $$\n    trap '' TERM\n    sleep 30\n$$\n"
        ),
    );
    let mut server = sb.server();
    server.wait_for("CMD_TIMEOUT");
    server.wait_for("CMD_KILL");
    assert!(server.wait_for("CMD_END").contains("signal=9"));
    assert!(
        server
            .wait_for("HOOK_OUT")
            .contains("failed: stubborn timeout")
    );
    server.stop();
}

#[test]
fn shutdown_kills_what_does_not_stop() {
    let sb = Sandbox::new("shutdown");
    sb.write(
        "tab",
        &format!(
            "{FAST}PCRON_SHUTDOWN_TIMEOUT=0.5\n\
             PCRON_ON_FAILURE=echo hook\n\
             stubborn: 0.1 $$\n    trap '' TERM\n    echo ready\n    sleep 30\n$$\n"
        ),
    );
    let mut server = sb.server();
    // a SIGTERM before the trap would just stop it
    server.wait_for("ready");

    let start = Instant::now();
    let (ok, log) = server.stop();
    assert!(ok);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    assert!(log.iter().any(|l| l.contains("CMD_KILL")), "{log:?}");
    // stopped by the shutdown, which is not a failure
    assert!(!log.iter().any(|l| l.contains("HOOK")), "{log:?}");
}

#[test]
fn reload_keeps_unchanged_commands() {
    let sb = Sandbox::new("reload");
    sb.write("tab", "a: 3600 $ true\nb: 3600 $ true\n");
    let mut server = sb.server();

    sb.write("tab", "b: 7200 $ true\n# reordered\na: 3600 $ true\n");
    sb.write("tab.d/c.tab", "c: 3600 $ true\n");
    server.signal(libc::SIGHUP);
    assert!(
        server
            .wait_for("RELOAD")
            .contains("kept=1, started=2, stopped=1")
    );

    sb.write("tab", "oops(\n");
    server.signal(libc::SIGHUP);
    assert!(
        server
            .wait_for("RELOAD_ERROR")
            .contains("keeping the previous tab")
    );
    server.stop();
}

#[test]
fn timers_survive_a_restart() {
    let sb = Sandbox::new("timers");
    sb.write("tab", "later: 3600 $ true\n");

    let mut server = sb.server();
    let sleep = server.wait_for("SLEEP");
    let until = sleep
        .split("until ")
        .nth(1)
        .unwrap()
        .trim_end_matches(") (later)");
    server.stop();

    let mut server = sb.server();
    let resume = server.wait_for("RESUME");
    assert!(
        resume.contains(&format!("until {until}")),
        "{resume} vs {sleep}"
    );
    server.stop();

    // a run that was missed while it was down happens right away
    let state = sb.dir.join("state/pcron");
    let file = std::fs::read_dir(&state)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(&file).unwrap();
    let next = text
        .split("\"next\": \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    std::fs::write(&file, text.replace(next, "2020-01-01T00:00:00+00:00")).unwrap();

    let mut server = sb.server();
    assert!(server.wait_for("RESUME").contains("running now"));
    server.wait_for("CMD_START");
    server.stop();
}

#[test]
fn timers_go_to_pcron_state() {
    let sb = Sandbox::new("pcron-state");
    sb.write("tab", "later: 3600 $ true\n");
    let state = sb.dir.join("my-timers.json");
    let env = [("PCRON_STATE", state.to_str().unwrap())];

    let mut server = sb.server_with(&env);
    server.wait_for("SLEEP");
    server.stop();
    assert!(state.exists());
    assert!(!sb.dir.join("state").exists(), "not in XDG_STATE_HOME");

    let mut server = sb.server_with(&env);
    server.wait_for("RESUME");
    server.stop();
}
