import * as path from "path";
import * as fs from "fs";
import * as https from "https";
import * as os from "os";
import { spawn } from "child_process";
import { workspace, ExtensionContext, ProgressLocation } from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";
import vscode from "vscode";
import lspVersionRaw from "../lsp-version.txt";

const LSP_VERSION = lspVersionRaw.trim();

let client: LanguageClient;

const getPlatformLabel = (): string | undefined => {
    switch (process.platform) {
        case "win32":
            return "windows-x86_64";

        case "darwin":
            switch (process.arch) {
                case "arm64":
                    return "macos-aarch64";
                case "x64":
                    return "macos-x86_64";
            }

        case "linux":
            if (process.arch === "x64") return "linux-x86_64";
    }

    return undefined;
};

const getBinaryName = (label: string): string => {
    const suffix = process.platform === "win32" ? ".exe" : "";
    return `rsml-lsp-${label}${suffix}`;
};

const findOnPath = (binaryName: string): string | undefined => {
    const pathEnv = process.env.PATH;

    if (!pathEnv) return undefined;

    for (const dir of pathEnv.split(path.delimiter)) {
        if (!dir) continue;

        const candidate = path.join(dir, binaryName);

        if (fs.existsSync(candidate)) return candidate;
    }

    return undefined;
};

const downloadToFile = (url: string, destPath: string): Promise<void> => {
    return new Promise((resolve, reject) => {
        const request = (currentUrl: string, redirectsLeft: number) => {
            https
                .get(currentUrl, (response) => {
                    const status = response.statusCode ?? 0;

                    if (
                        status >= 300 &&
                        status < 400 &&
                        response.headers.location
                    ) {
                        if (redirectsLeft <= 0) {
                            reject(
                                new Error(`too many redirects fetching ${url}`),
                            );
                            return;
                        }

                        response.resume();
                        request(response.headers.location, redirectsLeft - 1);
                        return;
                    }

                    if (status !== 200) {
                        reject(
                            new Error(`HTTP ${status} fetching ${currentUrl}`),
                        );
                        response.resume();
                        return;
                    }

                    const fileStream = fs.createWriteStream(destPath);
                    response.pipe(fileStream);

                    fileStream.on("finish", () =>
                        fileStream.close(() => resolve()),
                    );
                    fileStream.on("error", (err) => {
                        fs.promises.unlink(destPath).catch(() => {});
                        reject(err);
                    });
                })
                .on("error", reject);
        };

        request(url, 5);
    });
};

const pruneStaleVersions = async (
    parent: string,
    currentDir: string,
): Promise<void> => {
    let entries: string[];

    try {
        entries = await fs.promises.readdir(parent);
    } catch {
        return;
    }

    await Promise.all(
        entries.map(async (name) => {
            if (!name.startsWith("rsml-lsp-") || name === currentDir) return;

            const stale = path.join(parent, name);

            await fs.promises
                .rm(stale, { recursive: true, force: true })
                .catch(() => {});
        }),
    );
};

const downloadAndExtract = async (
    url: string,
    archiveName: string,
    installDir: string,
): Promise<void> => {
    await fs.promises.mkdir(installDir, { recursive: true });

    const tmpArchive = path.join(
        os.tmpdir(),
        `${archiveName}-${process.pid}-${Date.now()}.tar.gz`,
    );

    try {
        await vscode.window.withProgress(
            {
                location: ProgressLocation.Notification,
                title: `Downloading RSML LSP ${LSP_VERSION}…`,
                cancellable: false,
            },
            async () => {
                await downloadToFile(url, tmpArchive);
                await extractTarGz(tmpArchive, installDir);
            },
        );
    } finally {
        await fs.promises.unlink(tmpArchive).catch(() => {});
    }
};

const extractTarGz = (archivePath: string, destDir: string): Promise<void> => {
    return new Promise((resolve, reject) => {
        const child = spawn("tar", ["-xzf", archivePath, "-C", destDir], {
            stdio: ["ignore", "ignore", "pipe"],
        });

        let stderr = "";
        child.stderr.on("data", (chunk) => {
            stderr += chunk.toString();
        });

        child.on("error", reject);
        child.on("close", (code) => {
            if (code === 0) {
                resolve();
            } else {
                reject(
                    new Error(
                        `tar exited with code ${code}${stderr ? `: ${stderr.trim()}` : ""}`,
                    ),
                );
            }
        });
    });
};

const resolveServerPath = async (
    context: ExtensionContext,
): Promise<string> => {
    const label = getPlatformLabel();

    if (!label)
        throw new Error(
            `unsupported platform: ${process.platform} ${process.arch}`,
        );

    const binaryName = getBinaryName(label);

    const pathBinary = findOnPath(binaryName);

    if (pathBinary) return pathBinary;

    const sharedPath = path.join(
        context.extensionPath,
        "..",
        "shared",
        "servers",
        binaryName,
    );

    if (fs.existsSync(sharedPath)) return sharedPath;

    const storageRoot = context.globalStorageUri.fsPath;
    const installDirName = `rsml-lsp-${LSP_VERSION}`;
    const installDir = path.join(storageRoot, installDirName);
    const binaryPath = path.join(installDir, binaryName);

    await fs.promises.mkdir(storageRoot, { recursive: true });
    await pruneStaleVersions(storageRoot, installDirName);

    if (!fs.existsSync(binaryPath)) {
        const archiveName = `rsml-lsp-server-${label}.tar.gz`;
        const url = `https://github.com/rbx-rsml/rsml-lsp/releases/download/lsp-v${LSP_VERSION}/${archiveName}`;

        try {
            await downloadAndExtract(url, archiveName, installDir);
        } catch (err) {
            const message = err instanceof Error ? err.message : String(err);

            throw new Error(
                `failed to download ${archiveName}: ${message}\n\n` +
                    `Install manually by running: extensions/install-server.sh`,
            );
        }
    }

    if (process.platform !== "win32") {
        try {
            fs.chmodSync(binaryPath, 0o755);
        } catch {}
    }

    return binaryPath;
};

export async function activate(context: ExtensionContext) {
    const outputChannel = vscode.window.createOutputChannel("RSML LSP");
    outputChannel.appendLine("Activated RSML LSP");

    let serverModulePath: string;

    try {
        serverModulePath = await resolveServerPath(context);
    } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`RSML LSP: ${message}`);
        return;
    }

    outputChannel.appendLine(`Using server binary: ${serverModulePath}`);

    const serverOptions: ServerOptions = {
        run: { command: serverModulePath },
        debug: { command: serverModulePath, args: ["--debug"] },
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [
            { scheme: "file", language: "rsml" },
            { scheme: "file", pattern: "**/luaurc" },
            { scheme: "file", pattern: "**/*.luaurc" },
        ],
        synchronize: {
            fileEvents: workspace.createFileSystemWatcher(
                "**/{luaurc,*.luaurc,*.rsml}",
            ),
        },
    };

    client = new LanguageClient(
        "RsmlLanguageServer",
        "RSML Language Server",
        serverOptions,
        clientOptions,
    );
    client.start();

    context.subscriptions.push(
        vscode.commands.registerCommand("rsml.restart", async () => {
            context.subscriptions.forEach((x) => x.dispose());

            await deactivate();
            await activate(context);
        }),
    );
}

export function deactivate(): Thenable<void> | undefined {
    if (!client) return undefined;
    return client.stop();
}
