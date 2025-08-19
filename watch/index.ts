import { copyFile, watch } from "fs";
import { join } from "path";

const fileNamePrefix = "rsml-lsp"

const getFileNames = (): [inputFileName: string, outputFileName: string] => {
    const platform = process.platform
    const arch = process.arch

    switch (platform) {
        case "win32": return [ `${fileNamePrefix}.exe`, `${fileNamePrefix}-windows-x86_64.exe` ]

        case "darwin": switch (arch) {
            case "arm64": return [ fileNamePrefix, `${fileNamePrefix}-macos-aarch64` ]
            case "x64": return [ fileNamePrefix, `${fileNamePrefix}-macos-x86_64` ]
            default: throw new Error(`Unsupported architecture: ${arch}`)
        }

        case "linux": return [ fileNamePrefix, `${fileNamePrefix}-linux-x86_64` ]

        default: throw new Error(`Unsupported architecture: ${arch}`)
    }
}

const watchDir = "../target";
const [ inputFileName, outputFileName ] = getFileNames()

const debugInputPath = `debug/${inputFileName}`
const releaseInputPath = `release/${inputFileName}`

const outputPaths = [
    `../extensions/vscode/server/${outputFileName}`
]

console.log(`\x1b[34m👀 Watching "${watchDir}" for changes.`)

watch(watchDir, { recursive: true }, (eventType, filename) => {
    if (filename && eventType == "change" && (filename == debugInputPath || filename == releaseInputPath)) {
        const sourcePath = join(watchDir, filename)

        for (const outputPath of outputPaths) {
            copyFile(sourcePath, outputPath, (err) => {
                if (err) {
                    console.error(`\x1b[31m⚠️ Could not copy ${sourcePath} to ${outputPath}: `, err);
                } else {
                    console.log(`\x1b[32m✅ Copied ${sourcePath} to ${outputPath}`);
                }
            })
        }
    }
})