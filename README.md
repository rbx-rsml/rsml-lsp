# RSML

Language server and editor extensions for **RSML** (Roblox Style Management Language) — a styling language for Roblox UI inspired by CSS.

Learn more about RSML at [rsml.style](https://www.rsml.style), and about Roblox's underlying StyleSheet system on the [DevForum](https://devforum.roblox.com/t/how-to-use-robloxs-stylesheet-system/3346444).

## What's in this repo

| Path | Purpose |
| --- | --- |
| `src/` | The `rsml-lsp` language server (Rust, [`tower-lsp`](https://crates.io/crates/tower-lsp)) — diagnostics, hovers, completions, go-to-definition. |
| `extensions/vscode/` | VS Code extension. Bundles the language client; downloads the matching LSP binary on first launch. |
| `extensions/zed/` | Zed extension. Same model — registers the language and downloads the matching LSP binary. |

## Installation

**VS Code**: install [RSML LSP](https://marketplace.visualstudio.com/items?itemName=rbx-rsml.roblox-style-management-language) from the Marketplace, or search for "RSML" in the Extensions panel.

**Zed**: install "RSML" from the Zed extensions registry (`cmd+shift+x` → search "RSML").

## Building from source

The LSP server, VS Code extension, and Zed extension each have their own build instructions in their respective directories. The release workflows under `.github/workflows/` produce signed artifacts on tag push (`lsp-v*`, `vscode-v*`, `zed-v*`).

## License

MIT — see [LICENSE](./LICENSE).
