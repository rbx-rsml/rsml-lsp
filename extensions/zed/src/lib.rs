use std::fs;
use zed_extension_api::{self as zed, Result};

struct RsmlExtension {
    cached_binary_path: Option<String>,
}

const LSP_VERSION: &str = include_str!("../lsp-version.txt");

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

        let label = match (platform, arch) {
            (zed::Os::Mac, zed::Architecture::Aarch64) => "macos-aarch64",
            (zed::Os::Mac, zed::Architecture::X8664) => "macos-x86_64",
            (zed::Os::Linux, zed::Architecture::X8664) => "linux-x86_64",
            (zed::Os::Windows, zed::Architecture::X8664) => "windows-x86_64",
            _ => return Err(format!("unsupported platform: {platform:?} {arch:?}")),
        };

        let binary_suffix = if platform == zed::Os::Windows { ".exe" } else { "" };
        let binary_name = format!("rsml-lsp-{label}{binary_suffix}");

        if let Some(path) = worktree.which(&binary_name) {
            self.cached_binary_path = Some(path.clone());
            return Ok(path);
        }

        let shared_path = format!("../shared/servers/{binary_name}");

        if fs::metadata(&shared_path).map_or(false, |metadata| metadata.is_file()) {
            zed::make_file_executable(&shared_path)?;
            self.cached_binary_path = Some(shared_path.clone());
            return Ok(shared_path);
        }

        let archive_name = format!("rsml-lsp-server-{label}.zip");
        let download_url = format!(
            "https://github.com/rbx-rsml/rsml-lsp/releases/download/lsp-v{LSP_VERSION}/{archive_name}"
        );
        let install_dir = format!("rsml-lsp-{LSP_VERSION}");
        let binary_path = format!("{install_dir}/{binary_name}");

        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("rsml-lsp-") && name != install_dir {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }

        if !fs::metadata(&binary_path).map_or(false, |metadata| metadata.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::CheckingForUpdate,
            );

            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(&download_url, &install_dir, zed::DownloadedFileType::Zip)
                .map_err(|err| format!(
                    "failed to download {archive_name}: {err}\n\n\
                    Install manually by running: extensions/install-server.sh\n\
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
                ))?;

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
