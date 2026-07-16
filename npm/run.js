#!/usr/bin/env node
// Picks the bundled binary for the current platform/arch and execs it.
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const KEY = process.platform + '-' + process.arch;
const BINARIES = {
  "linux-x64": "run",
  "linux-arm64": "run",
  "darwin-arm64": "run",
  "darwin-x64": "run",
  "win32-x64": "run.exe",
  "win32-arm64": "run.exe"
};

const bin = BINARIES[KEY];
if (!bin) {
	console.error(`run: unsupported platform ${KEY}. Supported: ${Object.keys(BINARIES).join(', ')}`);
	process.exit(1);
}

const binPath = path.join(__dirname, KEY, bin);
const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
	console.error(`run: failed to spawn ${binPath}: ${result.error.message}`);
	process.exit(1);
}
process.exit(result.status == null ? 1 : result.status);
