use std::fs;
use zed_extension_api::{self as zed, Result};

struct RsmlExtension {
    cached_binary_path: Option<String>,
}

const GITHUB_RELEASE_URL: &str =
    "https://github.com/rbx-rsml/rsml-lsp/releases/latest/download";

impl RsmlExtension {
    fn language_server_binary_path(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).map_or(false, |metadata| metadata.is_file()) {
                return Ok(path.clone());
            }
        }

        let (platform, arch) = zed::current_platform();

        let binary_name = match (platform, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "rsml-lsp-macos-aarch64",
            (zed::Os::Mac, zed::Architecture::X8664) => "rsml-lsp-macos-x86_64",
            (zed::Os::Linux, zed::Architecture::X8664) => "rsml-lsp-linux-x86_64",
            (zed::Os::Windows, zed::Architecture::X8664) => "rsml-lsp-windows-x86_64.exe",
            _ => return Err(format!("unsupported platform: {platform:?} {arch:?}")),
        };

        // Try to find a pre-installed binary on PATH
        if let Some(path) = worktree.which(binary_name) {
            self.cached_binary_path = Some(path.clone());
            return Ok(path);
        }

        // Try downloading from GitHub releases
        let download_url = format!("{GITHUB_RELEASE_URL}/{binary_name}");
        let binary_path = format!("rsml-lsp/{binary_name}");

        if !fs::metadata(&binary_path).map_or(false, |metadata| metadata.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::CheckingForUpdate,
            );

            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            let download_result = zed::download_file(
                &download_url,
                &binary_path,
                zed::DownloadedFileType::Uncompressed,
            );

            if let Err(err) = download_result {
                return Err(format!(
                    "failed to download {binary_name}: {err}\n\n\
                    Install manually by running: extensions/zed/install-server.sh\n\
                    Then configure in Zed settings:\n\
                    {{\n  \
                      \"lsp\": {{\n    \
                        \"rsml-lsp\": {{\n      \
                          \"binary\": {{\n        \
                            \"path\": \"~/.local/bin/{binary_name}\"\n      \
                          }}\n    \
                        }}\n  \
                      }}\n\
                    }}"
                ));
            }

            zed::make_file_executable(&binary_path)?;
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for RsmlExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary_path =
            self.language_server_binary_path(language_server_id, worktree)?;

        Ok(zed::Command {
            command: binary_path,
            args: vec![],
            env: Default::default(),
        })
    }
}

zed::register_extension!(RsmlExtension);
