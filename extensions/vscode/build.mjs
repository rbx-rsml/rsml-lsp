import * as esbuild from "esbuild"
import { globby } from "globby";
import { spawnSync } from "child_process";

const inject = await globby([
    "src/**/*.{js,ts,tsx}",
    '!src/index.ts'
]);

const watch = process.argv.includes('--watch');
const isRelease = process.argv.includes('--release');

async function main() {
    const ctx = await esbuild.context({
        entryPoints:["src/index.ts"],
        inject: inject,
        bundle: true,
        minify: isRelease,
        keepNames: true,
        format: 'cjs',
        sourcemap: false,
        sourcesContent: false,
        platform: 'node',
        outfile: 'index.js',
        external: ['vscode'],
        drop: ['debugger'],
        loader: { '.txt': 'text' },
        logLevel: 'warning',
        plugins: [
            /* add to the end of plugins array */
            esbuildProblemMatcherPlugin
        ]
    });
    if (watch) {
        await ctx.watch();
    } else {
        await ctx.rebuild();
        await ctx.dispose();

        const result = spawnSync('bunx', ['@vscode/vsce', 'package', '--no-dependencies'], {
            stdio: 'inherit',
        });
        if (result.status !== 0) process.exit(result.status ?? 1);
    }
}

/**
 * @type {import('esbuild').Plugin}
 */
const esbuildProblemMatcherPlugin = {
    name: 'esbuild-problem-matcher',

    setup(build) {
        build.onStart(() => {
            console.log('[watch] build started');
        });
        build.onEnd(result => {
            result.errors.forEach(({ text, location }) => {
                console.error(`✘ [ERROR] ${text}`);
                if (location == null) return;
                console.error(`    ${location.file}:${location.line}:${location.column}:`);
            });
            console.log('[watch] build finished');
        });
    }
};

main().catch(e => {
    console.error(e);
    process.exit(1);
});