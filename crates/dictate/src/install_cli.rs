//! Puts this binary on the user's PATH as `~/.local/bin/<name>`, a symlink
//! into the app, so a terminal can run the CLI that ships inside
//! `Dictate.app`. The CLI only talks to the running app over its socket, so
//! permissions stay with the app.

use std::fs;
use std::io::ErrorKind;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;

/// Marks the PATH line this module adds, so it is added once.
const PATH_MARKER: &str = "# Added by Dictate: put ~/.local/bin on PATH";

/// Where the CLI is, or would be, installed, and what is there now.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct CliStatus {
    pub link: PathBuf,
    pub state: CliState,
    /// Whether new shells find `~/.local/bin`, as far as the shell's
    /// startup file shows.
    pub on_path: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CliState {
    /// The link points at this binary.
    Installed,
    NotInstalled,
    /// Another file has the name; installing would not replace it.
    Blocked,
}

/// What [`install`] changed.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct CliInstall {
    pub link: PathBuf,
    /// The shell startup file that gained the PATH line, if one did.
    pub startup_file: Option<PathBuf>,
}

/// The user's environment, read once so tests can supply their own.
pub struct Environment {
    home: PathBuf,
    exe: PathBuf,
    shell: Option<String>,
    path: Option<std::ffi::OsString>,
}

impl Environment {
    pub fn current() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is not set"))?;
        let exe = std::env::current_exe()?
            .canonicalize()
            .context("could not locate this binary")?;
        Ok(Self {
            home,
            exe,
            shell: std::env::var("SHELL").ok(),
            path: std::env::var_os("PATH"),
        })
    }

    fn bin_dir(&self) -> PathBuf {
        self.home.join(".local").join("bin")
    }

    fn link(&self) -> Result<PathBuf> {
        let name = self
            .exe
            .file_name()
            .ok_or_else(|| anyhow!("{} has no file name", self.exe.display()))?;
        Ok(self.bin_dir().join(name))
    }

    /// The startup file the PATH line goes in, and the line itself.
    fn startup_file(&self) -> (PathBuf, &'static str) {
        let shell = self
            .shell
            .as_deref()
            .and_then(|shell| Path::new(shell).file_name())
            .and_then(|name| name.to_str())
            .unwrap_or(if cfg!(target_os = "macos") {
                "zsh"
            } else {
                "bash"
            });
        let posix = "export PATH=\"$HOME/.local/bin:$PATH\"";
        match shell {
            "fish" => (
                self.home.join(".config/fish/conf.d/dictate.fish"),
                "fish_add_path -g $HOME/.local/bin",
            ),
            // Terminal on macOS opens login shells, which read .bash_profile.
            "bash" if cfg!(target_os = "macos") => (self.home.join(".bash_profile"), posix),
            "bash" => (self.home.join(".bashrc"), posix),
            _ => (self.home.join(".zshrc"), posix),
        }
    }

    fn on_path(&self) -> bool {
        let bin_dir = self.bin_dir();
        let in_environment = self
            .path
            .as_ref()
            .is_some_and(|path| std::env::split_paths(path).any(|entry| entry == bin_dir));
        // An app opened from Finder has launchd's PATH, not the shell's, so
        // the startup file decides there.
        in_environment
            || fs::read_to_string(self.startup_file().0)
                .is_ok_and(|contents| contents.contains(".local/bin"))
    }
}

pub fn status(environment: &Environment) -> Result<CliStatus> {
    let link = environment.link()?;
    let state = match fs::canonicalize(&link) {
        Ok(target) if target == environment.exe => CliState::Installed,
        Ok(_) if !is_symlink(&link) => CliState::Blocked,
        Ok(_) => CliState::NotInstalled,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // A dangling symlink, left by an app that moved, is replaced.
            CliState::NotInstalled
        }
        Err(error) => return Err(error).context(format!("could not inspect {}", link.display())),
    };
    Ok(CliStatus {
        link,
        state,
        on_path: environment.on_path(),
    })
}

/// Links this binary into `~/.local/bin`, replacing an older link but never
/// another file, and adds that directory to PATH in the shell's startup
/// file when it is missing. Running it again changes nothing.
pub fn install(environment: &Environment) -> Result<CliInstall> {
    let current = status(environment)?;
    let link = current.link;
    match current.state {
        CliState::Installed => {}
        CliState::Blocked => bail!(
            "{} is another file; move it away, then install again",
            link.display()
        ),
        CliState::NotInstalled => {
            fs::create_dir_all(environment.bin_dir())?;
            if is_symlink(&link) {
                fs::remove_file(&link)?;
            }
            symlink(&environment.exe, &link)
                .with_context(|| format!("could not create {}", link.display()))?;
        }
    }
    let startup_file = if current.on_path {
        None
    } else {
        let (file, path_line) = environment.startup_file();
        append_path_line(&file, path_line)?;
        Some(file)
    };
    Ok(CliInstall { link, startup_file })
}

fn append_path_line(file: &Path, line: &str) -> Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let existing = match fs::read_to_string(file) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context(format!("could not read {}", file.display())),
    };
    if existing.contains(PATH_MARKER) {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(format!("{separator}\n{PATH_MARKER}\n{line}\n").as_bytes())
        .with_context(|| format!("could not write {}", file.display()))
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn scratch(name: &str) -> (Scratch, Environment) {
        let root =
            std::env::temp_dir().join(format!("dictate-install-cli-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&root));
        let app = root.join("Dictate.app/Contents/MacOS");
        fs::create_dir_all(&app).expect("app dir");
        let exe = app.join("dictate");
        fs::write(&exe, "binary").expect("exe");
        let environment = Environment {
            home: root.join("home"),
            exe: exe.canonicalize().expect("exe path"),
            shell: Some("/bin/zsh".to_owned()),
            path: Some("/usr/bin:/bin".into()),
        };
        (Scratch(root), environment)
    }

    #[test]
    fn install_links_the_app_binary_and_adds_path_once() {
        let (_scratch, environment) = scratch("fresh");
        let zshrc = environment.home.join(".zshrc");
        fs::create_dir_all(&environment.home).expect("home");
        fs::write(&zshrc, "alias ll='ls -l'").expect("zshrc");

        let first = install(&environment).expect("install");
        assert_eq!(first.link, environment.home.join(".local/bin/dictate"));
        assert_eq!(first.startup_file.as_deref(), Some(zshrc.as_path()));
        assert_eq!(
            fs::canonicalize(&first.link).expect("link resolves"),
            environment.exe
        );
        assert_eq!(
            fs::read_to_string(&zshrc).expect("zshrc"),
            format!("alias ll='ls -l'\n\n{PATH_MARKER}\nexport PATH=\"$HOME/.local/bin:$PATH\"\n")
        );

        let again = install(&environment).expect("second install");
        assert_eq!(again.startup_file, None, "the PATH line is added once");
        let status = status(&environment).expect("status");
        assert_eq!((status.state, status.on_path), (CliState::Installed, true));
    }

    #[test]
    fn install_replaces_a_stale_link_but_not_another_file() {
        let (_scratch, environment) = scratch("stale");
        let bin = environment.home.join(".local/bin");
        fs::create_dir_all(&bin).expect("bin");
        symlink(
            Path::new("/nonexistent/Dictate.app/dictate"),
            &bin.join("dictate"),
        )
        .expect("stale link");
        install(&environment).expect("a stale link is replaced");
        assert_eq!(
            status(&environment).expect("status").state,
            CliState::Installed
        );

        fs::remove_file(bin.join("dictate")).expect("remove link");
        fs::write(bin.join("dictate"), "someone else's binary").expect("other file");
        assert_eq!(
            status(&environment).expect("status").state,
            CliState::Blocked
        );
        assert!(install(&environment).is_err());
        assert_eq!(
            fs::read_to_string(bin.join("dictate")).expect("other file"),
            "someone else's binary",
            "another file is never replaced"
        );
    }
}
