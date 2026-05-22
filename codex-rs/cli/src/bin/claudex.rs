use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitStatus;

const CLAUDEX_CODEX_BIN_ENV: &str = "CLAUDEX_CODEX_BIN";
const CLAUDEX_HOME_ENV: &str = "CLAUDEX_HOME";
const CLAUDEX_MARKER_ENV: &str = "CLAUDEX";
const CODEX_HOME_ENV: &str = "CODEX_HOME";

fn main() {
    match run() {
        Ok(status) => exit_with_status(status),
        Err(err) => {
            eprintln!("claudex: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<ExitStatus, String> {
    let claudex_home = resolve_claudex_home()?;
    fs::create_dir_all(&claudex_home).map_err(|err| {
        format!(
            "failed to create CLAUDEX_HOME {}: {err}",
            claudex_home.display()
        )
    })?;

    let codex_bin = resolve_codex_bin();
    Command::new(&codex_bin)
        .args(env::args_os().skip(1))
        .env(CODEX_HOME_ENV, &claudex_home)
        .env(CLAUDEX_MARKER_ENV, "1")
        .status()
        .map_err(|err| {
            format!(
                "failed to launch Codex binary {}: {err}",
                codex_bin.display()
            )
        })
}

fn resolve_claudex_home() -> Result<PathBuf, String> {
    if let Some(claudex_home) = non_empty_env(CLAUDEX_HOME_ENV) {
        return Ok(PathBuf::from(claudex_home));
    }

    user_home_dir()
        .map(|home| home.join(".claudex"))
        .ok_or_else(|| "failed to resolve home directory; set CLAUDEX_HOME".to_string())
}

fn resolve_codex_bin() -> PathBuf {
    if let Some(codex_bin) = non_empty_env(CLAUDEX_CODEX_BIN_ENV) {
        return PathBuf::from(codex_bin);
    }

    let executable_name = if cfg!(windows) { "codex.exe" } else { "codex" };
    if let Ok(current_exe) = env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let sibling = parent.join(executable_name);
        if sibling.is_file() {
            return sibling;
        }
    }

    PathBuf::from(executable_name)
}

fn non_empty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.as_os_str().is_empty())
}

fn user_home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| {
            let home_drive = env::var_os("HOMEDRIVE")?;
            let home_path = env::var_os("HOMEPATH")?;
            let mut home = PathBuf::from(home_drive);
            home.push(home_path);
            Some(home)
        })
    } else {
        env::var_os("HOME").map(PathBuf::from)
    }
}

fn exit_with_status(status: ExitStatus) -> ! {
    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            std::process::exit(128 + signal);
        }
    }

    std::process::exit(1);
}
