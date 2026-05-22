use std::env;
use std::ffi::OsStr;
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
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("claudex: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32, String> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == OsStr::new("app")) {
        run_claudex_app(args)?;
        return Ok(0);
    }

    run_codex(args).map(exit_code_from_status)
}

fn run_codex(args: Vec<OsString>) -> Result<ExitStatus, String> {
    let claudex_home = resolve_claudex_home()?;
    fs::create_dir_all(&claudex_home).map_err(|err| {
        format!(
            "failed to create CLAUDEX_HOME {}: {err}",
            claudex_home.display()
        )
    })?;

    let codex_bin = resolve_codex_bin();
    Command::new(&codex_bin)
        .args(args)
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run_claudex_app(args: Vec<OsString>) -> Result<(), String> {
    let app_args = parse_claudex_app_args(args.into_iter().skip(1))?;
    let workspace = std::fs::canonicalize(&app_args.path).unwrap_or(app_args.path);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("failed to create async runtime: {err}"))?;
    runtime
        .block_on(codex_cli::desktop_app::run_app_open_or_install(
            codex_cli::desktop_app::DesktopAppKind::Claudex,
            workspace,
            app_args.download_url_override,
        ))
        .map_err(|err| err.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn run_claudex_app(_args: Vec<OsString>) -> Result<(), String> {
    Err("Claudex Desktop launcher is not supported on this platform yet".to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct ClaudexAppArgs {
    path: PathBuf,
    download_url_override: Option<String>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn parse_claudex_app_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<ClaudexAppArgs, String> {
    let mut path = None;
    let mut download_url_override = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        if arg == OsStr::new("--download-url") {
            let Some(value) = args.next() else {
                return Err("--download-url requires a URL".to_string());
            };
            download_url_override = Some(
                value
                    .into_string()
                    .map_err(|_| "--download-url must be valid UTF-8".to_string())?,
            );
        } else if let Some(value) = arg
            .to_str()
            .and_then(|value| value.strip_prefix("--download-url="))
        {
            download_url_override = Some(value.to_string());
        } else if arg.to_string_lossy().starts_with('-') {
            return Err(format!(
                "unknown claudex app option {}",
                arg.to_string_lossy()
            ));
        } else if path.replace(PathBuf::from(arg)).is_some() {
            return Err("claudex app accepts at most one PATH".to_string());
        }
    }

    Ok(ClaudexAppArgs {
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        download_url_override,
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

fn exit_code_from_status(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    1
}
