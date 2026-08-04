import * as esbuild from 'esbuild';

const watch = process.argv.includes('--watch');

/** @type {import('esbuild').BuildOptions} */
const config = {
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'dist/extension.js',
  external: ['vscode'],
  format: 'cjs',
  platform: 'node',
  target: 'node18',
  sourcemap: watch,
  minify: !watch,
};

if (watch) {
  const ctx = await esbuild.context(config);
  await ctx.watch();
  console.log('[esbuild] watching for changes...');
} else {
  await esbuild.build(config);
  console.log('[esbuild] build complete');
}
