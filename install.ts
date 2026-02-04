#!/usr/bin/env bun
// Marlang installation script
// Compiles the CLI and installs to ~/.marlang

import * as fs from 'fs';
import * as path from 'path';

const HOME = process.env.HOME || process.env.USERPROFILE;
if (!HOME) {
  console.error('Error: Could not determine home directory');
  process.exit(1);
}

const MARLANG_HOME = path.join(HOME, '.marlang');
const BIN_DIR = path.join(MARLANG_HOME, 'bin');
const STDLIB_DIR = path.join(MARLANG_HOME, 'stdlib');
const BINARY_PATH = path.join(BIN_DIR, 'mar');

const SOURCE_DIR = import.meta.dir;
const SOURCE_STDLIB = path.join(SOURCE_DIR, 'stdlib');

async function main() {
  console.log('Installing Marlang...\n');

  // 1. Create directories
  console.log(`Creating ${MARLANG_HOME}`);
  fs.mkdirSync(BIN_DIR, { recursive: true });

  // 2. Copy stdlib
  console.log(`Copying stdlib to ${STDLIB_DIR}`);
  if (fs.existsSync(STDLIB_DIR)) {
    fs.rmSync(STDLIB_DIR, { recursive: true });
  }
  copyDir(SOURCE_STDLIB, STDLIB_DIR);

  // 3. Compile binary
  console.log(`Compiling CLI to ${BINARY_PATH}`);
  const result = Bun.spawnSync({
    cmd: ['bun', 'build', '--compile', path.join(SOURCE_DIR, 'index.ts'), '--outfile', BINARY_PATH],
    cwd: SOURCE_DIR,
    stdout: 'inherit',
    stderr: 'inherit',
  });

  if (result.exitCode !== 0) {
    console.error('Error: Compilation failed');
    process.exit(1);
  }

  // 4. Print success message
  console.log('\n✓ Marlang installed successfully!\n');
  console.log('Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):');
  console.log(`\n  export PATH="${BIN_DIR}:$PATH"\n`);
  console.log('Then run: mar --help');
}

function copyDir(src: string, dest: string) {
  fs.mkdirSync(dest, { recursive: true });
  const entries = fs.readdirSync(src, { withFileTypes: true });

  for (const entry of entries) {
    const srcPath = path.join(src, entry.name);
    const destPath = path.join(dest, entry.name);

    if (entry.isDirectory()) {
      copyDir(srcPath, destPath);
    } else {
      fs.copyFileSync(srcPath, destPath);
    }
  }
}

main();
