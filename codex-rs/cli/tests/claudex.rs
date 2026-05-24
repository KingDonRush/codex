#![cfg(unix)]

use anyhow::Result;
use pretty_assertions::assert_eq;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn claudex_uses_explicit_isolated_home() -> Result<()> {
    let temp = TempDir::new()?;
    let fake_codex = write_fake_codex(temp.path())?;
    let claudex_home = temp.path().join("claudex-home");

    let output = Command::new(codex_utils_cargo_bin::cargo_bin("claudex")?)
        .env("CLAUDEX_CODEX_BIN", &fake_codex)
        .env("CLAUDEX_HOME", &claudex_home)
        .env(
            "CODEX_HOME",
            temp.path().join("codex-home-that-should-not-leak"),
        )
        .arg("exec")
        .arg("hello")
        .output()?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "CODEX_HOME={}\nCLAUDEX=1\nARGS=exec hello\n",
            claudex_home.display()
        )
    );
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert!(claudex_home.is_dir());

    Ok(())
}

#[test]
fn claudex_defaults_to_dot_claudex_under_home() -> Result<()> {
    let temp = TempDir::new()?;
    let fake_codex = write_fake_codex(temp.path())?;
    let home = temp.path().join("home");
    fs::create_dir(&home)?;
    let claudex_home = home.join(".claudex");

    let output = Command::new(codex_utils_cargo_bin::cargo_bin("claudex")?)
        .env("CLAUDEX_CODEX_BIN", &fake_codex)
        .env("HOME", &home)
        .env_remove("CLAUDEX_HOME")
        .arg("--version")
        .output()?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!(
            "CODEX_HOME={}\nCLAUDEX=1\nARGS=--version\n",
            claudex_home.display()
        )
    );
    assert_eq!(String::from_utf8(output.stderr)?, "");
    assert!(claudex_home.is_dir());

    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn claudex_app_does_not_forward_to_codex_when_unsupported() -> Result<()> {
    let temp = TempDir::new()?;
    let fake_codex = write_fake_codex(temp.path())?;
    let claudex_home = temp.path().join("claudex-home");

    let output = Command::new(codex_utils_cargo_bin::cargo_bin("claudex")?)
        .env("CLAUDEX_CODEX_BIN", &fake_codex)
        .env("CLAUDEX_HOME", &claudex_home)
        .arg("app")
        .output()?;

    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "");
    assert_eq!(
        String::from_utf8(output.stderr)?,
        "claudex: Claudex Desktop launcher is not supported on this platform yet\n"
    );
    assert!(!claudex_home.exists());

    Ok(())
}

fn write_fake_codex(root: &std::path::Path) -> Result<std::path::PathBuf> {
    let fake_codex = root.join("fake-codex");
    fs::write(
        &fake_codex,
        "#!/bin/sh\nprintf 'CODEX_HOME=%s\\n' \"$CODEX_HOME\"\nprintf 'CLAUDEX=%s\\n' \"$CLAUDEX\"\nprintf 'ARGS=%s\\n' \"$*\"\n",
    )?;
    let mut permissions = fs::metadata(&fake_codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions)?;
    Ok(fake_codex)
}
