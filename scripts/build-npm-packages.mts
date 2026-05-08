#!/usr/bin/env node
// Build the single npm package for runfile.
//
// Layout:
//   - 1 package (@runfile/cli) — bundles all 6 platform binaries under
//     `bin/<key>/run[.exe]` and ships a tiny stub at `bin/run.js` that
//     dispatches at runtime by `process.platform + '-' + process.arch`.
//
// Tradeoff vs. the optionalDependencies pattern (esbuild/swc/biome/turbo):
// every install pulls all six binaries (~30–60 MB compressed total) instead
// of one, but the install path is bulletproof — there's no
// `--ignore-scripts` / `--no-optional` / corporate-mirror / sandbox failure
// mode where the platform-specific package gets skipped and the wrapper
// can't find its binary.
//
// Usage:
//   node scripts/build-npm-packages.mts \
//     --version 0.1.0 \
//     --artifacts target/distrib \
//     --out npm-dist
//
// Reads tarballs/zips named like dist produces:
//   runfile-cli-aarch64-apple-darwin.tar.xz
//   runfile-cli-x86_64-pc-windows-msvc.zip
//   etc.

import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const SCOPE = '@runfile';
const PUBLIC_NAME = 'cli';
const REPO_URL = 'https://github.com/Skiley/runfile';

interface Platform {
	/** Matches `${process.platform}-${process.arch}` at runtime in the stub. */
	readonly key: string;
	/** Filename of the dist artifact (relative to the artifacts dir). */
	readonly archive: string;
	/** Filename of the binary inside the artifact (`run` on Unix, `run.exe` on Windows). */
	readonly bin: string;
}

const PLATFORMS: readonly Platform[] = [
	{ key: 'darwin-arm64', archive: 'runfile-cli-aarch64-apple-darwin.tar.xz', bin: 'run' },
	{ key: 'darwin-x64', archive: 'runfile-cli-x86_64-apple-darwin.tar.xz', bin: 'run' },
	{ key: 'linux-arm64', archive: 'runfile-cli-aarch64-unknown-linux-musl.tar.xz', bin: 'run' },
	{ key: 'linux-x64', archive: 'runfile-cli-x86_64-unknown-linux-musl.tar.xz', bin: 'run' },
	{ key: 'win32-arm64', archive: 'runfile-cli-aarch64-pc-windows-msvc.zip', bin: 'run.exe' },
	{ key: 'win32-x64', archive: 'runfile-cli-x86_64-pc-windows-msvc.zip', bin: 'run.exe' },
];

// --- arg parsing --------------------------------------------------------

interface BuildArgs {
	readonly version: string;
	readonly artifacts: string;
	readonly out: string;
}

function parseArgs(argv: readonly string[]): BuildArgs {
	const args: { version: string | null; artifacts: string | null; out: string | null } = {
		version: null,
		artifacts: null,
		out: null,
	};
	for (let i = 0; i < argv.length; i++) {
		const a = argv[i];
		if (a === '--version') args.version = argv[++i];
		else if (a === '--artifacts') args.artifacts = argv[++i];
		else if (a === '--out') args.out = argv[++i];
		else throw new Error(`unknown arg: ${a}`);
	}
	for (const k of ['version', 'artifacts', 'out'] as const) {
		if (!args[k]) throw new Error(`missing required --${k}`);
	}
	// All three keys are non-null after the check loop above; the `as` is
	// purely for the type system since TS can't narrow through the loop.
	return args as BuildArgs;
}

// --- archive extraction -------------------------------------------------

function extractBinary(archivePath: string, binName: string, destDir: string): void {
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
		const found = execFileSync('find', [tmp, '-name', binName, '-type', 'f'], { encoding: 'utf8' })
			.trim()
			.split('\n')[0];
		if (!found) throw new Error(`binary ${binName} not found in ${archivePath}`);
		copyFileSync(found, join(destDir, binName));
		rmSync(tmp, { recursive: true, force: true });
	} else {
		throw new Error(`unsupported archive format: ${archivePath}`);
	}
}

// --- package builder ----------------------------------------------------

interface WritePackageOptions {
	readonly outDir: string;
	readonly version: string;
	readonly artifactsDir: string;
}

interface WritePackageResult {
	readonly pkgName: string;
	readonly pkgDir: string;
}

function writePackage({ outDir, version, artifactsDir }: WritePackageOptions): WritePackageResult {
	const pkgDir = join(outDir, PUBLIC_NAME);
	const binDir = join(pkgDir, 'bin');
	mkdirSync(binDir, { recursive: true });

	// Extract each platform's binary into bin/<key>/<bin>.
	for (const platform of PLATFORMS) {
		const archivePath = join(artifactsDir, platform.archive);
		if (!existsSync(archivePath)) {
			throw new Error(`missing artifact: ${archivePath}`);
		}
		const platDir = join(binDir, platform.key);
		extractBinary(archivePath, platform.bin, platDir);
		// Best-effort exec bit — needed on Unix when the publisher runs on Linux
		// (the source archives are .tar.xz so perms are usually preserved, but
		// `unzip`-extracted .exe files don't carry a Unix mode and chmod is a
		// no-op on Windows). The .exe doesn't need it.
		if (platform.bin === 'run') {
			chmodSync(join(platDir, platform.bin), 0o755);
		}
		console.log(`  bundled ${platform.key}`);
	}

	const pkgJson = {
		name: `${SCOPE}/${PUBLIC_NAME}`,
		version,
		description: 'Runfile CLI — an easy-to-use, batteries-included task runner',
		repository: REPO_URL,
		homepage: REPO_URL,
		license: 'MIT',
		bin: { run: 'bin/run.js' },
		files: ['bin'],
		engines: { node: '>=16' },
	};
	writeFileSync(join(pkgDir, 'package.json'), JSON.stringify(pkgJson, null, 2) + '\n');

	// Runtime stub. Resolves the bundled binary by `process.platform + '-' +
	// process.arch`, then execs it. Zero dependencies, no postinstall.
	const stub = `#!/usr/bin/env node
// Picks the bundled binary for the current platform/arch and execs it.
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const KEY = process.platform + '-' + process.arch;
const BINARIES = ${JSON.stringify(
		Object.fromEntries(PLATFORMS.map((p) => [p.key, p.bin])),
		null,
		2,
	)};

const bin = BINARIES[KEY];
if (!bin) {
	console.error(\`runfile: unsupported platform \${KEY}. Supported: \${Object.keys(BINARIES).join(', ')}\`);
	process.exit(1);
}

const binPath = path.join(__dirname, KEY, bin);
const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
	console.error(\`runfile: failed to spawn \${binPath}: \${result.error.message}\`);
	process.exit(1);
}
process.exit(result.status == null ? 1 : result.status);
`;
	writeFileSync(join(binDir, 'run.js'), stub, { mode: 0o755 });

	// README in the package so npmjs.com has something to show.
	writeFileSync(join(pkgDir, 'README.md'), `# ${SCOPE}/${PUBLIC_NAME}\n\nSee ${REPO_URL} for documentation.\n`);

	return { pkgName: `${SCOPE}/${PUBLIC_NAME}`, pkgDir };
}

// --- main ---------------------------------------------------------------

function main(): void {
	const { version, artifacts, out } = parseArgs(process.argv.slice(2));
	const artifactsAbs = resolve(artifacts);
	const outAbs = resolve(out);

	if (existsSync(outAbs)) rmSync(outAbs, { recursive: true, force: true });
	mkdirSync(outAbs, { recursive: true });

	console.log(`Building ${SCOPE}/${PUBLIC_NAME} v${version}`);
	console.log(`  artifacts: ${artifactsAbs}`);
	console.log(`  out:       ${outAbs}`);

	const { pkgName } = writePackage({ outDir: outAbs, version, artifactsDir: artifactsAbs });
	console.log(`\nDone. Built ${pkgName} in ${outAbs}/${PUBLIC_NAME}/`);
}

try {
	main();
} catch (e) {
	const message = e instanceof Error ? e.message : String(e);
	console.error(`error: ${message}`);
	process.exit(1);
}
