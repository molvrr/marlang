// Build script for browser bundle
// Usage: bun build-browser.ts

import { resolve } from 'path';

const result = await Bun.build({
  entrypoints: ['./browser.ts'],
  outdir: './dist',
  target: 'browser',
  minify: false, // Keep readable for debugging
  sourcemap: 'external',
  naming: '[name].js',
  // Replace bun:ffi with our browser shim
  plugins: [
    {
      name: 'ffi-shim',
      setup(build) {
        build.onResolve({ filter: /^bun:ffi$/ }, () => {
          return { path: resolve('./ffi-shim.ts') };
        });
      },
    },
  ],
});

if (!result.success) {
  console.error('Build failed:');
  for (const log of result.logs) {
    console.error(log);
  }
  process.exit(1);
}

console.log('Build successful!');
console.log('Output files:');
for (const output of result.outputs) {
  console.log(`  ${output.path} (${(output.size / 1024).toFixed(1)} KB)`);
}
