#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopAppKind {
    Codex,
    Claudex,
}

impl DesktopAppKind {
    pub(crate) fn product_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claudex => "Claudex",
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn app_bundle_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex.app",
            Self::Claudex => "Claudex.app",
        }
    }
}

/// Run the app install/open logic for the current OS.
#[cfg(target_os = "macos")]
pub async fn run_app_open_or_install(
    kind: DesktopAppKind,
    workspace: std::path::PathBuf,
    download_url_override: Option<String>,
) -> anyhow::Result<()> {
    mac::run_mac_app_open_or_install(kind, workspace, download_url_override).await
}

/// Run the app install/open logic for the current OS.
#[cfg(target_os = "windows")]
pub async fn run_app_open_or_install(
    kind: DesktopAppKind,
    workspace: std::path::PathBuf,
    download_url_override: Option<String>,
) -> anyhow::Result<()> {
    windows::run_windows_app_open_or_install(kind, workspace, download_url_override).await
}
