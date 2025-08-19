import * as path from "path"
import * as fs from "fs"
import { workspace, ExtensionContext } from "vscode"
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node"

import vscode from "vscode"

let client: LanguageClient;

const basePath = "./server"
const fileNamePrefix = "rsml-lsp-"

const getServerModulePath = (context: ExtensionContext): string | undefined => {
    switch (process.platform) {
        case "win32": return context.asAbsolutePath(path.join(basePath, `${fileNamePrefix}windows-x86_64.exe`))

        case "darwin": switch (process.arch) {
            case "arm64": return context.asAbsolutePath(path.join(basePath, `${fileNamePrefix}macos-aarch64`))
            case "x64": return context.asAbsolutePath(path.join(basePath, `${fileNamePrefix}macos-x86_64`))
        }

        case "linux": return context.asAbsolutePath(path.join(basePath, `${fileNamePrefix}linux-x86_64`))
    }

    return undefined
}

export function activate(context: ExtensionContext) {
    vscode.window.showInformationMessage("Activated RSML LSP")

    let serverModulePath = getServerModulePath(context)
    if (!serverModulePath) return vscode.window.showErrorMessage("The RSML LSP is not supported on your platform")
    if (!fs.existsSync(serverModulePath)) return vscode.window.showErrorMessage("Could not locate the RSML LSP. (This is a bug and should be reported [here](https://github.com/rbx-rsml/rsml-lsp/issues)).")

    const serverOptions: ServerOptions = {
        run: { command: serverModulePath },
        debug: { command: serverModulePath, args: ["--debug"] }  // if you want debug mode,
        
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