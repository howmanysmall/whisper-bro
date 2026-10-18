import { readFile, writeFile } from "node:fs/promises";
import { defineConfig } from "bumpp";
import { exec } from "tinyexec";

const CARGO_MANIFEST = "Cargo.toml";
const CARGO_LOCKFILE = "Cargo.lock";
const PACKAGE_NAME = "yap";
const RELEASE_BRANCH = "main";

// Matches the `version` key of the [package] table, stopping at the next table.
const PACKAGE_VERSION = /^(?<prefix>\[package\]\n(?:(?!\[)[^\n]*\n)*?version = ")(?<version>[^"]+)(?<suffix>")/mv;

async function runAsync(command: string, parameters: ReadonlyArray<string>): Promise<string> {
	const { stdout } = await exec(command, parameters, { throwOnError: true });
	return stdout;
}

async function readCargoVersionAsync(): Promise<string> {
	const manifest = await readFile(CARGO_MANIFEST, "utf8");
	const version = PACKAGE_VERSION.exec(manifest)?.groups?.version;
	if (version === undefined) {
		throw new Error(`${CARGO_MANIFEST} has no [package] version`);
	}
	return version;
}

async function getUnpushedTagsAsync(): Promise<ReadonlyArray<string>> {
	const stdout = await runAsync("git", ["push", "--tags", "--dry-run", "--porcelain"]);

	const unpushedTags = new Array<string>();
	let size = 0;

	for (const line of stdout.split("\n")) {
		if (!line.startsWith("*\trefs/tags/")) continue;
		unpushedTags[size++] = line.split("\t")[1]?.split(":")[0]?.slice("refs/tags/".length) ?? line;
	}

	return unpushedTags;
}

const branch = (await runAsync("git", ["branch", "--show-current"])).trim();
if (branch !== RELEASE_BRANCH) {
	throw new Error(`releases are cut from ${RELEASE_BRANCH}, not ${branch || "a detached HEAD"}`);
}

const dirtyFiles = await runAsync("git", ["status", "--porcelain"]);
if (dirtyFiles.trim().length > 0) {
	throw new Error(`git working tree is not clean; commit or stash first:\n${dirtyFiles}`);
}

const { version: packageVersion } = JSON.parse(await readFile("package.json", "utf8")) as { version: string };
const cargoVersion = await readCargoVersionAsync();
if (packageVersion !== cargoVersion) {
	throw new Error(`package.json is at ${packageVersion} but ${CARGO_MANIFEST} is at ${cargoVersion}`);
}

const strayTags = await getUnpushedTagsAsync();
if (strayTags.length > 0) {
	throw new Error(
		`local tags missing from the remote would be pushed with the release: ${strayTags.join(", ")}\n` +
			"push or delete them first (git tag -d <tag>)",
	);
}

const configuration = defineConfig({
	all: true,
	commit: "chore(release): v%s",
	async execute({ state }): Promise<void> {
		const { newVersion } = state;

		const manifest = await readFile(CARGO_MANIFEST, "utf8");
		await writeFile(CARGO_MANIFEST, manifest.replace(PACKAGE_VERSION, `$<prefix>${newVersion}$<suffix>`));
		await runAsync("cargo", ["update", "--workspace", "--offline"]);

		const lockfile = await readFile(CARGO_LOCKFILE, "utf8");
		if ((await readCargoVersionAsync()) !== newVersion) {
			throw new Error(`${CARGO_MANIFEST} was not bumped to ${newVersion}`);
		}
		if (!lockfile.includes(`name = "${PACKAGE_NAME}"\nversion = "${newVersion}"`)) {
			throw new Error(`${CARGO_LOCKFILE} was not bumped to ${newVersion}`);
		}
	},
	files: ["package.json"],
	noGitCheck: false,
	printCommits: true,
	push: true,
	tag: "v%s",
});

export default configuration;
