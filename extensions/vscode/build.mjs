import * as esbuild from "esbuild"
import { globby } from "globby";

const inject = await globby([
    "src/**/*.{js,ts,tsx}"
]);

const release = process.argv.includes('--release');
const watch = process.argv.includes('--watch');

async function main() {
    const ctx = await esbuild.context({
        stdin: { contents: '' },
        inject: inject,
        bundle: true,
        format: 'cjs',
        minify: release,
        sourcemap: !release,
        sourcesContent: false,
        platform: 'node',
        outfile: 'out.js',
        external: ['vscode'],
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