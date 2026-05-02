#!/usr/bin/env node
// Build the npm packages for runfile using the optionalDependencies pattern.
//
// Layout:
//   - 6 per-platform packages (@skiley/runfile-<os>-<arch>) — each contains
//     just the binary plus a minimal package.json with `os` and `cpu` so
//     npm refuses to install on the wrong platform.
//   - 1 wrapper package (@skiley/runfile) — a tiny stub that resolves the
//     correct platform package at runtime and execs the binary. Lists all
//     6 platform packages as optionalDependencies; npm installs only the
//     matching one.
//
// Result: 0 transitive dependencies, no postinstall download. Same pattern
// as esbuild, @biomejs/biome, @swc/core, lightningcss, turbo.
//
// Usage:
//   node scripts/build-npm-packages.mjs \
//     --version 0.1.0 \
//     --artifacts target/distrib \
//     --out npm-dist
//
// Reads tarballs/zips named like dist produces:
//   runfile-cli-aarch64-apple-darwin.tar.xz
//   runfile-cli-x86_64-pc-windows-msvc.zip
//   etc.

import { execFileSync } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync, existsSync, copyFileSync, chmodSync } from 'node:fs';
import { join, resolve } from 'node:path';

const SCOPE = '@skiley';
const PUBLIC_NAME = 'runfile';

// Platform table: npm-style key -> dist artifact info
const PLATFORMS = [
	{
		key: 'darwin-arm64',
		os: 'darwin',
		cpu: 'arm64',
		archive: 'runfile-cli-aarch64-apple-darwin.tar.xz',
		bin: 'run',
	},
	{
		key: 'darwin-x64',
		os: 'darwin',
		cpu: 'x64',
		archive: 'runfile-cli-x86_64-apple-darwin.tar.xz',
		bin: 'run',
	},
	{
		key: 'linux-arm64',
		os: 'linux',
		cpu: 'arm64',
		archive: 'runfile-cli-aarch64-unknown-linux-musl.tar.xz',
		bin: 'run',
	},
	{
		key: 'linux-x64',
		os: 'linux',
		cpu: 'x64',
		archive: 'runfile-cli-x86_64-unknown-linux-musl.tar.xz',
		bin: 'run',
	},
	{
		key: 'win32-arm64',
		os: 'win32',
		cpu: 'arm64',
		archive: 'runfile-cli-aarch64-pc-windows-msvc.zip',
		bin: 'run.exe',
	},
	{
		key: 'win32-x64',
		os: 'win32',
		cpu: 'x64',
		archive: 'runfile-cli-x86_64-pc-windows-msvc.zip',
		bin: 'run.exe',
	},
];

// --- arg parsing --------------------------------------------------------

function parseArgs(argv) {
	const args = { version: null, artifacts: null, out: null };
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i];
		if (a === '--version') args.version = argv[++i];
		else if (a === '--artifacts') args.artifacts = argv[++i];
		else if (a === '--out') args.out = argv[++i];
		else throw new Error(`unknown arg: ${a}`);
	}
	for (const k of ['version', 'artifacts', 'out']) {
		if (!args[k]) throw new Error(`missing required --${k}`);
	}
	return args;
}

// --- archive extraction -------------------------------------------------

function extractBinary(archivePath, binName, destDir) {
	mkdirSync(destDir, { recursive: true });
	if (archivePath.endsWith('.tar.xz')) {
		// dist tarballs contain a top-level dir like runfile-cli-<target>/run
		// Extract the binary directly into destDir, stripping the leading dir.
		execFileSync('tar', ['-xJf', archivePath, '--strip-components=1', '-C', destDir], { stdio: 'inherit' });
	} else if (archivePath.endsWith('.zip')) {
		// Zip layout: <zipname>/run.exe — extract everything then move
		const tmp = join(destDir, '.tmp');
		mkdirSync(tmp, { recursive: true });
		execFileSync('unzip', ['-q', archivePath, '-d', tmp], { stdio: 'inherit' });
		// Find the binary anywhere under tmp and move it to destDir
		const found = execFileSync('find', [tmp, '-name', binName, '-type', 'f'], { encoding: 'utf8' }).trim().split('\n')[0];
		if (!found) throw new Error(`binary ${binName} not found in ${archivePath}`);
		copyFileSync(found, join(destDir, binName));
		rmSync(tmp, { recursive: true, force: true });
	} else {
		throw new Error(`unsupported archive format: ${archivePath}`);
	}
}

// --- package builders ---------------------------------------------------

function writePlatformPackage({ outDir, version, platform }) {
	const pkgName = `${SCOPE}/${PUBLIC_NAME}-${platform.key}`;
	const pkgDir = join(outDir, `${PUBLIC_NAME}-${platform.key}`);
	mkdirSync(pkgDir, { recursive: true });

	const pkgJson = {
		name: pkgName,
		version,
		description: `${PUBLIC_NAME} binary for ${platform.os}-${platform.cpu}`,
		repository: `https://github.com/Skiley/${PUBLIC_NAME}`,
		license: 'MIT',
		os: [platform.os],
		cpu: [platform.cpu],
		files: [platform.bin],
	};
	writeFileSync(join(pkgDir, 'package.json'), JSON.stringify(pkgJson, null, 2) + '\n');

	const binDest = join(pkgDir, platform.bin);
	chmodSync(binDest, 0o755);

	return { pkgName, pkgDir };
}

function writeWrapperPackage({ outDir, version }) {
	const pkgDir = join(outDir, PUBLIC_NAME);
	const binDir = join(pkgDir, 'bin');
	mkdirSync(binDir, { recursive: true });

	const optionalDependencies = {};
	for (const p of PLATFORMS) {
		optionalDependencies[`${SCOPE}/${PUBLIC_NAME}-${p.key}`] = version;
	}

	const pkgJson = {
		name: `${SCOPE}/${PUBLIC_NAME}`,
		version,
		description: 'Runfile CLI — an easy-to-use, batteries-included task runner',
		repository: `https://github.com/Skiley/${PUBLIC_NAME}`,
		homepage: `https://github.com/Skiley/${PUBLIC_NAME}`,
		license: 'MIT',
		bin: { run: 'bin/run.js' },
		optionalDependencies,
		engines: { node: '>=16' },
	};
	writeFileSync(join(pkgDir, 'package.json'), JSON.stringify(pkgJson, null, 2) + '\n');

	// Runtime stub. Resolves the matching platform package via
	// require.resolve(), then execs the binary inside it. Zero dependencies.
	const stub = `#!/usr/bin/env node
// Resolves the matching @skiley/${PUBLIC_NAME}-<platform> package and execs the binary.
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const KEY = process.platform + '-' + process.arch;
const PLATFORMS = ${JSON.stringify(
		Object.fromEntries(
			PLATFORMS.map(p => [p.key, { pkg: `${SCOPE}/${PUBLIC_NAME}-${p.key}`, bin: p.bin }]),
		),
		null,
		2,
	)};

const entry = PLATFORMS[KEY];
if (!entry) {
	console.error(\`runfile: unsupported platform \${KEY}. Supported: \${Object.keys(PLATFORMS).join(', ')}\`);
	process.exit(1);
}

let binPath;
try {
	const pkgJson = require.resolve(entry.pkg + '/package.json');
	binPath = path.join(path.dirname(pkgJson), entry.bin);
} catch {
	console.error(
		\`runfile: missing platform package \${entry.pkg}.\\n\` +
		\`This usually means npm skipped optionalDependencies. Reinstall without --no-optional, --omit=optional, or --ignore-scripts.\`,
	);
	process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
	console.error(\`runfile: failed to spawn \${binPath}: \${result.error.message}\`);
	process.exit(1);
}
process.exit(result.status == null ? 1 : result.status);
`;
	writeFileSync(join(binDir, 'run.js'), stub, { mode: 0o755 });

	// README in the wrapper so npmjs.com has something to show.
	writeFileSync(
		join(pkgDir, 'README.md'),
		`# ${SCOPE}/${PUBLIC_NAME}\n\nSee https://github.com/Skiley/${PUBLIC_NAME} for documentation.\n`,
	);

	return { pkgName: `${SCOPE}/${PUBLIC_NAME}`, pkgDir };
}

// --- main ---------------------------------------------------------------

function main() {
	const { version, artifacts, out } = parseArgs(process.argv.slice(2));
	const artifactsAbs = resolve(artifacts);
	const outAbs = resolve(out);

	if (existsSync(outAbs)) rmSync(outAbs, { recursive: true, force: true });
	mkdirSync(outAbs, { recursive: true });

	console.log(`Building npm packages for version ${version}`);
	console.log(`  artifacts: ${artifactsAbs}`);
	console.log(`  out:       ${outAbs}`);

	for (const platform of PLATFORMS) {
		const archivePath = join(artifactsAbs, platform.archive);
		if (!existsSync(archivePath)) {
			throw new Error(`missing artifact: ${archivePath}`);
		}
		const pkgDir = join(outAbs, `${PUBLIC_NAME}-${platform.key}`);
		mkdirSync(pkgDir, { recursive: true });
		extractBinary(archivePath, platform.bin, pkgDir);
		const { pkgName } = writePlatformPackage({ outDir: outAbs, version, platform });
		console.log(`  built ${pkgName}`);
	}

	const { pkgName: wrapperName } = writeWrapperPackage({ outDir: outAbs, version });
	console.log(`  built ${wrapperName} (wrapper)`);

	console.log(`\nDone. Packages in ${outAbs}/`);
	console.log(`Publish order: platform packages first, wrapper last.`);
}

try {
	main();
} catch (e) {
	console.error(`error: ${e.message}`);
	process.exit(1);
}
