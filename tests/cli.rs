mod common;

use common::{Sandbox, stderr, stdout};
use std::os::unix::fs::PermissionsExt;

#[test]
fn list_shows_every_file_of_the_tab() {
    let sb = Sandbox::new("list");
    sb.write("tab", "web: exp(1/60) $ curl example.com\n");
    sb.write(
        "tab.d/backup.tab",
        "backup: 24*3600 @ mon-fri 9-18 $ restic backup\n",
    );

    let out = sb.run(&["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let table = stdout(&out);
    assert!(table.starts_with("FILE "), "{table}");
    assert!(table.contains("tab.d/backup.tab"), "{table}");
    assert!(table.contains("86400"), "{table}");

    let out = sb.run(&["list", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<_> = json["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web", "backup"]);
}

#[test]
fn simulate() {
    let sb = Sandbox::new("simulate");
    sb.write("tab", "hourly: 3600 $ true\n");
    let out = sb.run(&["list", "--simulate", "1d", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["runs"].as_array().unwrap().len(), 23);
}

#[test]
fn errors_say_where() {
    let sb = Sandbox::new("errors");
    sb.write("tab", "60 $ ok\n");
    sb.write("tab.d/bad.tab", "\nexp(1 $ oops\n");
    let out = sb.run(&["list"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("bad.tab") && err.contains("line 2"), "{err}");

    let sb = Sandbox::new("no-tab");
    let out = sb.run(&["list"]);
    assert!(stderr(&out).contains("cannot find the tab"));
}

// an "editor" that writes $CONTENT
fn editor(sb: &Sandbox) -> String {
    let path = sb.dir.join("editor");
    std::fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$CONTENT\" > \"$1\"\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

#[test]
fn edit_installs_only_valid_tabs() {
    let sb = Sandbox::new("edit");
    let editor = editor(&sb);
    let edit = |args: &[&str], content: &str| {
        sb.cmd(args)
            .env("EDITOR", &editor)
            .env("CONTENT", content)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };

    let out = edit(&["edit"], "a: 60 $ true");
    assert!(out.status.success(), "{}", stderr(&out));
    let out = edit(&["edit", "more"], "b: 60 $ true");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(sb.dir.join("tab.d/more.tab").exists());

    // invalid, or clashing with another file: discarded (without a terminal to ask)
    for content in ["exp(", "a: 1 $ again"] {
        let out = edit(&["edit", "more"], content);
        assert!(!out.status.success());
        let more = std::fs::read_to_string(sb.dir.join("tab.d/more.tab")).unwrap();
        assert_eq!(more, "b: 60 $ true\n");
    }
    // and no copies are left behind
    let left: Vec<_> = std::fs::read_dir(sb.dir.join("tab.d"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["more.tab"]);
}
