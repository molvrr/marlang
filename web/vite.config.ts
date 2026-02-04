import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { resolve } from 'path';

export default defineConfig({
  plugins: [solid()],
  resolve: {
    alias: {
      // Allow importing from parent marlang source
      '@marlang': resolve(__dirname, '..'),
      // Shim bun:ffi for browser
      'bun:ffi': resolve(__dirname, '../ffi-shim.ts'),
    },
  },
  server: {
    port: 3000,
  },
});
