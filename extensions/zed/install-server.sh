#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
INSTALL_DIR="$HOME/.local/bin"

# Detect platform and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS-$ARCH" in
    Darwin-arm64)   BINARY_NAME="rsml-lsp-macos-aarch64" ;;
    Darwin-x86_64)  BINARY_NAME="rsml-lsp-macos-x86_64" ;;
    Linux-x86_64)   BINARY_NAME="rsml-lsp-linux-x86_64" ;;
    *)
        echo "error: unsupported platform: $OS $ARCH"
        exit 1
        ;;
esac

echo "Building rsml-lsp (release)..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"

echo "Building tree-sitter WASM grammar..."
GRAMMAR_DIR="$SCRIPT_DIR/grammars/rsml"
if command -v ~/.cargo/bin/tree-sitter &> /dev/null; then
    TREE_SITTER=~/.cargo/bin/tree-sitter
else
    TREE_SITTER=tree-sitter
fi

if $TREE_SITTER build --wasm -o "$SCRIPT_DIR/grammars/rsml.wasm" "$GRAMMAR_DIR" 2>&1; then
    echo "Built rsml.wasm"
else
    echo "error: failed to build rsml.wasm"
    exit 1
fi

mkdir -p "$INSTALL_DIR"
cp "$REPO_ROOT/target/release/rsml-lsp" "$INSTALL_DIR/$BINARY_NAME"
chmod +x "$INSTALL_DIR/$BINARY_NAME"

echo "Installed $BINARY_NAME to $INSTALL_DIR"

if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    SHELL_NAME="$(basename "$SHELL")"
    case "$SHELL_NAME" in
        zsh)  PROFILE="$HOME/.zshrc" ;;
        bash) PROFILE="$HOME/.bashrc" ;;
        *)    PROFILE="$HOME/.profile" ;;
    esac

    printf '\nexport PATH="%s:$PATH"\n' "$INSTALL_DIR" >> "$PROFILE"
    echo "Added $INSTALL_DIR to PATH in $PROFILE"
    echo "Run 'source $PROFILE' or restart your shell to apply."
fi
