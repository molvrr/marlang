# Web Playground - WASM Port Needed

The web playground currently depends on the archived TypeScript implementation (`ts-archive/browser.ts`), which provided parsing, type checking, and evaluation for the browser via Bun's bundler.

Since the TypeScript implementation has been archived, the web playground is broken and needs to be ported to use the Rust implementation compiled to WebAssembly (WASM).

## What needs to happen

1. Add a WASM build target to the Rust port (`port/`) using `wasm-pack` or `wasm-bindgen`
2. Expose the same API surface that `browser.ts` provided: `parse()`, `typeCheck()`, `run()`, `formatValue()`, `buildTypeMap()`, `getBuiltinReference()`
3. Update the SolidJS frontend (`web/src/`) to import from the WASM module instead of `@marlang/browser`
4. Update `web/vite.config.ts` to load the WASM module instead of aliasing to the TS source

## Previous architecture

- `browser.ts` bundled the full TS pipeline (lexer, parser, interpreter, type checker) for the browser
- `ffi-shim.ts` stubbed out `bun:ffi` for the browser environment
- `build-browser.ts` used `Bun.build()` to produce `dist/browser.js`
- `web/vite.config.ts` aliased `@marlang` to the repo root and shimmed `bun:ffi`
- `web/src/App.tsx` imported `* as Marlang from '@marlang/browser'`
