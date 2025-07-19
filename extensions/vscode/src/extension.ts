import * as path from "path"
import * as fs from "fs"
import { workspace, ExtensionContext } from "vscode"
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node"

import vscode from "vscode"

let client: LanguageClient;

const basePath = "./server"
const serverModuleName = "rsml-lsp"

const getServerModulePath = (context: ExtensionContext): string | undefined => {
    switch (process.platform) {
        case "win32": return context.asAbsolutePath(path.join(basePath, `${serverModuleName}-windows-x86_64.exe`))

        case "darwin": switch (process.arch) {
            case "arm64": return context.asAbsolutePath(path.join(basePath, `${serverModuleName}-macos-aarch64`))
            case "x64": return context.asAbsolutePath(path.join(basePath, `${serverModuleName}-macos-x86_64`))
        }

        case "linux": return context.asAbsolutePath(path.join(basePath, `${serverModuleName}-linux-x86_64`))
    }

    return undefined
}

const outputChannel = vscode.window.createOutputChannel('RSML');

export function activate(context: ExtensionContext) {
    let serverModulePath = getServerModulePath(context)
    if (!serverModulePath) return vscode.window.showErrorMessage("Could not locate the LSP file!")

    const serverOptions: ServerOptions = {
        run: { command: serverModulePath },
        debug: { command: serverModulePath, args: ["--debug"] }  // if you want debug mode
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "rsml" }],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher("**/*.rsml")
        }
    };

    client = new LanguageClient("RsmlLanguageServer", "RSML Language Server", serverOptions, clientOptions);
    client.start();
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) return undefined
    return client.stop();
}