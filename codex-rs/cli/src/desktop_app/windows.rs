use anyhow::Context as _;
use std::path::Path;
use std::path::PathBuf;
use tokio::process::Command;

use super::DesktopAppKind;

const CODEX_WINDOWS_INSTALLER_URL: &str =
    "https://get.microsoft.com/installer/download/9PLM9XGG6VKS?cid=website_cta_psi";
const CODEX_MICROSOFT_STORE_WEB_URL: &str = "https://apps.microsoft.com/detail/9plm9xgg6vks";

pub async fn run_windows_app_open_or_install(
    kind: DesktopAppKind,
    workspace: PathBuf,
    download_url_override: Option<String>,
) -> anyhow::Result<()> {
    let product_name = kind.product_name();
    if let Some(app_id) = find_app_id(kind).await? {
        eprintln!("Opening {product_name} Desktop...");
        open_installed_app(&app_id).await?;
        eprintln!(
            "In {product_name} Desktop, open workspace {workspace}.",
            workspace = display_workspace_path(&workspace)
        );
        return Ok(());
    }

    if let Some(download_url) = download_url_override.as_deref() {
        eprintln!("{product_name} Desktop not found; opening Windows installer...");
        open_url(download_url).await?;
    } else if kind == DesktopAppKind::Codex {
        eprintln!("{product_name} Desktop not found; opening Windows installer...");
        if open_url(CODEX_WINDOWS_INSTALLER_URL).await.is_err() {
            open_url(CODEX_MICROSOFT_STORE_WEB_URL).await?;
        }
    } else {
        anyhow::bail!("{product_name} Desktop not found; install it or pass --download-url");
    }

    eprintln!(
        "After installing {product_name} Desktop, open workspace {workspace}.",
        workspace = display_workspace_path(&workspace)
    );
    Ok(())
}

async fn find_app_id(kind: DesktopAppKind) -> anyhow::Result<Option<String>> {
    let product_name = kind.product_name();
    let output = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(format!(
            "Get-StartApps -Name '{product_name}' | Select-Object -First 1 -ExpandProperty AppID"
        ))
        .output()
        .await
        .context("failed to invoke `powershell.exe`")?;

    if !output.status.success() {
        return Ok(None);
    }

    let app_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if app_id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(app_id))
    }
}

async fn open_installed_app(app_id: &str) -> anyhow::Result<()> {
    let target = format!("shell:AppsFolder\\{app_id}");
    open_shell_target(&target).await
}

async fn open_url(url: &str) -> anyhow::Result<()> {
    let status = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-Command")
        .arg("& { param($target) Start-Process -FilePath $target }")
        .arg(url)
        .status()
        .await
        .with_context(|| format!("failed to open {url}"))?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to open {url} with {status}");
    }
}

async fn open_shell_target(target: &str) -> anyhow::Result<()> {
    // Explorer can successfully hand off shell targets and still return exit code 1.
    let _status = Command::new("explorer.exe")
        .arg(target)
        .status()
        .await
        .with_context(|| format!("failed to open {target}"))?;

    Ok(())
}

fn display_workspace_path(workspace: &Path) -> String {
    let path = workspace.display().to_string();
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{path}")
    } else if let Some(path) = path.strip_prefix(r"\\?\") {
        path.to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::display_workspace_path;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn display_workspace_path_removes_windows_extended_prefix() {
        assert_eq!(
            display_workspace_path(Path::new(r"\\?\C:\Users\fcoury\code\codex")),
            r"C:\Users\fcoury\code\codex"
        );
    }

    #[test]
    fn display_workspace_path_preserves_unc_prefix() {
        assert_eq!(
            display_workspace_path(Path::new(r"\\?\UNC\server\share\codex")),
            r"\\server\share\codex"
        );
    }

    #[test]
    fn display_workspace_path_leaves_regular_paths_unchanged() {
        assert_eq!(
            display_workspace_path(Path::new(r"C:\Users\fcoury\code\codex")),
            r"C:\Users\fcoury\code\codex"
        );
    }
}
