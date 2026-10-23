use anyhow::{Context, Result, bail, ensure};
use rand::Rng;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, ErrorKind, IsTerminal, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use tokio::{
    process::Command,
    signal::unix::{Signal, SignalKind, signal},
};

use crate::SHELL;
use crate::file;

// like visudo: edit a copy, check it (with the rest of the tab), and only then put it in
// place; `name` picks tab.d/NAME.tab instead of the tab file
pub async fn main_edit(tab: &Path, name: Option<&str>) -> Result<()> {
    let editor = find_editor()?;
    let target = match name {
        None => tab.to_path_buf(),
        Some(name) => {
            let name = name.strip_suffix(".tab").unwrap_or(name);
            ensure!(
                !name.is_empty() && !name.starts_with('.') && !name.contains('/'),
                "`{name}` is not a valid file name"
            );
            file::dropin_dir(tab).join(format!("{name}.tab"))
        }
    };
    let target: &Path = &target;
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot create directory {}", dir.display()))?;
    }

    let original = read_existing(target)?;
    let mut copy = EditCopy::create(target, original.as_deref())?;

    // the editor owns the terminal; don't let ^C (or a hangup) kill us and leave the copy behind
    let _signals = catch_signals()?;

    loop {
        run_editor(&editor, &copy.path).await?;

        let text = fs::read_to_string(&copy.path)
            .with_context(|| format!("cannot read {}", copy.path.display()))?;
        if Some(text.as_str()) == original.as_deref() || (original.is_none() && text.is_empty()) {
            eprintln!("no changes, {} was not modified", target.display());
            return Ok(());
        }

        match check(tab, target, &text) {
            Ok(_) => {
                // nothing locks the tab while it is edited, so check nobody else changed it
                if read_existing(target)? != original {
                    copy.keep();
                    bail!(
                        "{} changed while you were editing it, so it was not replaced; \
                         your version is in {}",
                        target.display(),
                        copy.path.display()
                    );
                }
                copy.install()?;
                eprintln!(
                    "installed {} (a running server picks it up with `pkill -HUP -x pcron`)",
                    target.display()
                );
                return Ok(());
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                if !ask_edit_again()? {
                    bail!("changes discarded, {} was not modified", target.display());
                }
            }
        }
    }
}

// the whole tab, with `text` as the contents of `target`
fn check(tab: &Path, target: &Path, text: &str) -> Result<()> {
    let mut files = file::tab_files(tab)?;
    if !files.iter().any(|f| f == target) {
        files.push(target.to_path_buf());
    }
    // in the order read uses: the tab file first, then the rest by name
    files.sort_by_key(|f| (f != tab, f.clone()));
    let texts = files
        .into_iter()
        .map(|f| {
            let contents = if f == target {
                text.to_string()
            } else {
                fs::read_to_string(&f).with_context(|| format!("cannot read {}", f.display()))?
            };
            Ok((f, contents))
        })
        .collect::<Result<Vec<_>>>()?;
    file::parse_files(tab, &texts).map(drop)
}

// $VISUAL, $EDITOR, or else vi or ed, as POSIX tools do
fn find_editor() -> Result<String> {
    for var in ["VISUAL", "EDITOR"] {
        match env::var(var) {
            Ok(editor) if !editor.trim().is_empty() => return Ok(editor),
            _ => {}
        }
    }
    ["vi", "ed"]
        .into_iter()
        .find(|prog| in_path(prog))
        .map(String::from)
        .context("no editor found: set $VISUAL or $EDITOR (neither vi nor ed is in $PATH)")
}

fn in_path(prog: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| {
        fs::metadata(dir.join(prog))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

fn read_existing(tab: &Path) -> Result<Option<String>> {
    match fs::read_to_string(tab) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read tab {}", tab.display())),
    }
}

async fn run_editor(editor: &str, path: &Path) -> Result<()> {
    let status = Command::new(SHELL)
        .arg("-c")
        .arg(format!(r#"exec {editor} "$1""#))
        .arg("sh")
        .arg(path)
        .status()
        .await
        .with_context(|| format!("cannot launch editor `{editor}`"))?;
    ensure!(
        status.success(),
        "editor `{editor}` failed ({status}), changes discarded"
    );
    Ok(())
}

// a caught signal does nothing here, and the editor still gets the default handlers on exec
fn catch_signals() -> Result<Vec<Signal>> {
    [
        SignalKind::interrupt(),
        SignalKind::quit(),
        SignalKind::hangup(),
        SignalKind::terminate(),
    ]
    .into_iter()
    .map(|kind| signal(kind).context("cannot set up signal handlers"))
    .collect()
}

fn ask_edit_again() -> Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    loop {
        eprint!("What now? (e)dit again, e(x)it without saving [e]: ");
        let mut answer = String::new();
        if io::stdin().lock().read_line(&mut answer)? == 0 {
            eprintln!();
            return Ok(false);
        }
        match answer.trim() {
            "" | "e" => return Ok(true),
            "x" => return Ok(false),
            _ => {}
        }
    }
}

/// the copy being edited; it lives next to the tab so that installing it is an atomic
/// rename, and has a random name so that concurrent (or crashed) edits don't get in the way
struct EditCopy {
    path: PathBuf,
    tab: PathBuf,
    /// don't remove it on drop (it was installed, or kept for the user)
    keep: bool,
}

impl EditCopy {
    fn create(tab: &Path, contents: Option<&str>) -> Result<Self> {
        let name = tab
            .file_name()
            .with_context(|| format!("{} is not a file path", tab.display()))?;
        let mut rng = rand::rng();
        let (path, mut f) = loop {
            let path = tab.with_file_name(format!(
                ".{}.{:08x}.tmp",
                name.to_string_lossy(),
                rng.random::<u32>()
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(f) => break (path, f),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(e).with_context(|| format!("cannot create {}", path.display()));
                }
            }
        };
        let copy = Self {
            path,
            tab: tab.to_path_buf(),
            keep: false,
        };

        // keep the tab's permissions (it may hold secrets)
        if contents.is_some() {
            let perms = fs::metadata(tab)?.permissions();
            fs::set_permissions(&copy.path, perms)?;
        }
        f.write_all(contents.unwrap_or_default().as_bytes())
            .with_context(|| format!("cannot write {}", copy.path.display()))?;
        Ok(copy)
    }

    fn install(&mut self) -> Result<()> {
        File::open(&self.path)
            .and_then(|f| f.sync_all())
            .and_then(|()| fs::rename(&self.path, &self.tab))
            .with_context(|| format!("cannot install {}", self.tab.display()))?;
        self.keep = true;
        Ok(())
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for EditCopy {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_copy() {
        let dir = env::temp_dir().join(format!("pcron-edit-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let tab = dir.join("tab");
        fs::write(&tab, "const(1) $ a\n").unwrap();
        fs::set_permissions(&tab, fs::Permissions::from_mode(0o600)).unwrap();

        let leftovers = || {
            fs::read_dir(&dir)
                .unwrap()
                .filter(|e| e.as_ref().unwrap().file_name() != "tab")
                .count()
        };

        let mut copy = EditCopy::create(&tab, Some("const(1) $ a\n")).unwrap();
        let name = copy.path.file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            name.starts_with(".tab.") && name.ends_with(".tmp"),
            "{name}"
        );
        let mode = fs::metadata(&copy.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // another edit at the same time gets its own copy
        let other = EditCopy::create(&tab, None).unwrap();
        assert_ne!(other.path, copy.path);
        drop(other);

        fs::write(&copy.path, "const(2) $ b\n").unwrap();
        copy.install().unwrap();
        drop(copy);
        assert_eq!(fs::read_to_string(&tab).unwrap(), "const(2) $ b\n");
        assert_eq!(leftovers(), 0);

        // a discarded edit leaves the tab alone and cleans up
        let copy = EditCopy::create(&tab, Some("x")).unwrap();
        fs::write(&copy.path, "garbage").unwrap();
        drop(copy);
        assert_eq!(fs::read_to_string(&tab).unwrap(), "const(2) $ b\n");
        assert_eq!(leftovers(), 0);

        // a kept copy survives
        let mut copy = EditCopy::create(&tab, Some("x")).unwrap();
        copy.keep();
        let path = copy.path.clone();
        drop(copy);
        assert!(path.exists());

        fs::remove_dir_all(&dir).unwrap();
    }
}
