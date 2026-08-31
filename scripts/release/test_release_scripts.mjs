#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import crypto from "node:crypto";
import { createServer } from "node:http";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);
const releaseDirectory = path.dirname(fileURLToPath(import.meta.url));
const projectDirectory = path.resolve(releaseDirectory, "../..");
const allocatorScript = path.join(releaseDirectory, "allocate_nightly_version.sh");
const buildScript = path.join(releaseDirectory, "build_macos_release.sh");
const tagVerifierScript = path.join(releaseDirectory, "verify_tag_target.sh");
const manifestScript = path.join(releaseDirectory, "create_manifest.mjs");
const publisherScript = path.join(releaseDirectory, "publish_release.mjs");
const reuseVerifierScript = path.join(releaseDirectory, "verify_release_reuse.mjs");
const workflowPath = path.join(releaseDirectory, "..", "..", ".github", "workflows", "release.yml");
const gitSha = "a".repeat(40);
const screenshotSourceGitSha = "b".repeat(40);
const introducedScreenshotSourceGitSha = "83dab42ef945d26b8e01ba48f7ce17f6bcfead63";
const cargoPackageVersion = "0.1.9";
const notarySubmissionId = "00000000-0000-4000-8000-000000000000";
const png = Buffer.from("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489", "hex");

function allocateNightlyVersion(cargoBaseVersion, releaseNames) {
  return execFileAsync("bash", [
    "-c",
    'printf "%s" "$1" | "$2" "$3"',
    "cadence-allocator-test",
    releaseNames,
    allocatorScript,
    cargoBaseVersion,
  ]).then(({ stdout }) => stdout.trim());
}

async function createInputDirectory(version) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-manifest-test-"));
  const appPath = path.join(directory, "Cadence.app");
  const contentsPath = path.join(appPath, "Contents");
  const executablePath = path.join(contentsPath, "MacOS", "Cadence");
  const shortVersion = version.split("-", 1)[0];
  const buildNumber = version === shortVersion ? shortVersion : version.slice(version.lastIndexOf(".") + 1);
  await fs.mkdir(path.dirname(executablePath), { recursive: true });
  await fs.mkdir(path.join(contentsPath, "Resources"), { recursive: true });
  await fs.writeFile(
    path.join(contentsPath, "Info.plist"),
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Cadence</string>
  <key>CFBundleExecutable</key>
  <string>Cadence</string>
  <key>CFBundleIdentifier</key>
  <string>org.portalsurfer.cadence</string>
  <key>CFBundleName</key>
  <string>Cadence</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${shortVersion}</string>
  <key>CFBundleVersion</key>
  <string>${buildNumber}</string>
</dict>
</plist>
`,
  );
  await fs.writeFile(executablePath, "synthetic arm64 Cadence executable\n");
  await fs.chmod(executablePath, 0o755);
  await fs.writeFile(path.join(contentsPath, "PkgInfo"), "APPL????");
  await fs.writeFile(path.join(contentsPath, "Resources", "Cadence.icns"), "synthetic Cadence icon\n");
  await execFileAsync("/usr/bin/ditto", [
    "-c",
    "-k",
    "--sequesterRsrc",
    "--keepParent",
    appPath,
    path.join(directory, `cadence-v${version}-macos-arm64.zip`),
  ]);
  await fs.rm(appPath, { recursive: true, force: true });
  await fs.writeFile(path.join(directory, "cadence-default-ui-1594x987.png"), png);
  await fs.writeFile(path.join(directory, "CHANGELOG.md"), "# Test release\n");
  return directory;
}

async function createManifest(directory, version, channel, { env } = {}) {
  const args = [
    manifestScript,
    "--output-dir", directory,
    "--version", version,
    "--build-id", "cadence-test-build",
    "--git-sha", gitSha,
    "--screenshot-source-git-sha", screenshotSourceGitSha,
    "--released-at", "2026-08-09T00:00:00Z",
    "--team-id", "TEAM123456",
    "--notary-submission-id", notarySubmissionId,
  ];
  if (channel) args.push("--channel", channel);
  await execFileAsync(process.execPath, args, env ? { env } : undefined);
  return JSON.parse(await fs.readFile(path.join(directory, "cadence-release-manifest.json"), "utf8"));
}

async function createBufferedReadGuard(filePath) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-buffered-read-guard-"));
  const guardPath = path.join(directory, "guard.cjs");
  await fs.writeFile(guardPath, [
    'const fs = require("node:fs");',
    'const fsPromises = require("node:fs/promises");',
    'const path = require("node:path");',
    "",
    "const target = path.resolve(process.env.CADENCE_TEST_BUFFERED_READ_PATH);",
    "function rejectBufferedRead(filePath) {",
    "  if (typeof filePath === \"string\" && path.resolve(filePath) === target) {",
    "    throw new Error(\"buffered read guard rejected \" + target);",
    "  }",
    "}",
    "",
    "const originalPromisesReadFile = fsPromises.readFile;",
    "fsPromises.readFile = function guardedPromisesReadFile(filePath, ...args) {",
    "  rejectBufferedRead(filePath);",
    "  return originalPromisesReadFile.call(this, filePath, ...args);",
    "};",
    "const originalReadFile = fs.readFile;",
    "fs.readFile = function guardedReadFile(filePath, ...args) {",
    "  rejectBufferedRead(filePath);",
    "  return originalReadFile.call(this, filePath, ...args);",
    "};",
    "const originalReadFileSync = fs.readFileSync;",
    "fs.readFileSync = function guardedReadFileSync(filePath, ...args) {",
    "  rejectBufferedRead(filePath);",
    "  return originalReadFileSync.call(this, filePath, ...args);",
    "};",
    "",
  ].join("\n"));
  const nodeOptions = [process.env.NODE_OPTIONS, "--require=" + guardPath]
    .filter((value) => value)
    .join(" ");
  return {
    environment: {
      ...process.env,
      CADENCE_TEST_BUFFERED_READ_PATH: filePath,
      NODE_OPTIONS: nodeOptions,
    },
    cleanup: () => fs.rm(directory, { recursive: true, force: true }),
  };
}

function forceBufferedRead(filePath, environment) {
  return execFileAsync(process.execPath, [
    "-e",
    "require('node:fs/promises').readFile(process.argv[1])",
    filePath,
  ], { env: environment });
}

async function startReleaseServer({ failStageName } = {}) {
  const requests = [];
  const server = createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      const body = Buffer.concat(chunks);
      requests.push({
        method: request.method,
        url: request.url,
        headers: request.headers,
        body,
      });
      if (request.method === "GET" && request.url === "/plugins/api/v1/products/cadence/releases") {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ artifact_kind: "application", release_upload: { manifest_schema_versions: [2] } }));
        return;
      }
      if (request.method === "PUT" && request.url?.includes("/staging/files/")) {
        const name = decodeURIComponent(request.url.split("/").at(-1));
        if (name === failStageName) {
          response.writeHead(500, { "Content-Type": "text/plain" });
          response.end("synthetic staging failure\n");
          return;
        }
        response.writeHead(200);
        response.end();
        return;
      }
      if (request.method === "PUT" && request.url?.endsWith("/commit")) {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(JSON.stringify({ committed: true }));
        return;
      }
      response.writeHead(404);
      response.end("not found\n");
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.equal(typeof address, "object");
  return {
    endpoint: `http://127.0.0.1:${address.port}`,
    requests,
    close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}

function publishRelease(directory, endpoint, options = {}) {
  return execFileAsync(process.execPath, [
    publisherScript,
    "--manifest", path.join(directory, "cadence-release-manifest.json"),
    "--root", directory,
    "--endpoint", endpoint,
    "--token", "fixture-release-token",
  ], options);
}

async function createFetchGuard() {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-fetch-guard-"));
  const guardPath = path.join(directory, "guard.cjs");
  await fs.writeFile(guardPath, 'globalThis.fetch = async () => { throw new Error("network activity"); };\n');
  return {
    environment: {
      ...process.env,
      NODE_OPTIONS: [process.env.NODE_OPTIONS, `--require=${guardPath}`]
        .filter((value) => value)
        .join(" "),
    },
    cleanup: () => fs.rm(directory, { recursive: true, force: true }),
  };
}

async function createReuseFixture() {
  const version = "0.1.0";
  const directory = await createInputDirectory(version);
  const manifest = await createManifest(directory, version);
  await writeReuseSums(directory, manifest);
  const commandStubs = await createReuseCommandStubs();
  return {
    directory,
    manifest,
    version,
    commandStubs,
    environment: {
      ...process.env,
      PATH: `${commandStubs}${path.delimiter}${process.env.PATH || ""}`,
    },
  };
}

async function createReuseCommandStubs() {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-reuse-command-stubs-"));
  await fs.writeFile(path.join(directory, "lipo"), `#!/bin/sh
set -eu
[ "$#" -eq 2 ] && [ "$1" = "-archs" ] && [ -f "$2" ]
printf '%s\\n' "\${CADENCE_TEST_LIPO_ARCHS:-arm64}"
`);
  await fs.writeFile(path.join(directory, "codesign"), `#!/bin/sh
set -eu
last_argument=""
for argument in "$@"; do last_argument="$argument"; done
[ "$1" = "--verify" ] && [ -e "$last_argument" ]
if [ "\${CADENCE_TEST_CODESIGN_FAILURE:-0}" = "1" ]; then
  echo "synthetic codesign failure" >&2
  exit 1
fi
`);
  await fs.chmod(path.join(directory, "lipo"), 0o755);
  await fs.chmod(path.join(directory, "codesign"), 0o755);
  return directory;
}

async function writeReuseSums(directory, manifest) {
  const descriptorNames = [
    manifest.artifacts[0].name,
    manifest.screenshot.name,
    manifest.changelog.name,
  ];
  const sums = [];
  for (const name of descriptorNames) {
    const bytes = await fs.readFile(path.join(directory, name));
    sums.push(`${crypto.createHash("sha256").update(bytes).digest("hex")}  ${name}`);
  }
  await fs.writeFile(path.join(directory, "SHA256SUMS.txt"), `${sums.join("\n")}\n`);
}

async function refreshReuseArtifactDescriptor(fixture, bytes) {
  const artifact = fixture.manifest.artifacts[0];
  await fs.writeFile(path.join(fixture.directory, artifact.name), bytes);
  artifact.size_bytes = bytes.length;
  artifact.sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
  await fs.writeFile(
    path.join(fixture.directory, "cadence-release-manifest.json"),
    `${JSON.stringify(fixture.manifest, null, 2)}\n`,
  );
  await writeReuseSums(fixture.directory, fixture.manifest);
}

function verifyReuseFixture(fixture, overrides = {}) {
  const { env: environmentOverrides = {}, ...valueOverrides } = overrides;
  const values = {
    version: "0.1.0",
    channel: "stable",
    buildId: "cadence-test-build",
    sourceSha: gitSha,
    repository: "PORTALSURFER/cadence",
    ...valueOverrides,
  };
  return execFileAsync(process.execPath, [
    reuseVerifierScript,
    "--root", fixture.directory,
    "--version", values.version,
    "--channel", values.channel,
    "--build-id", values.buildId,
    "--source-sha", values.sourceSha,
    "--repository", values.repository,
  ], { env: { ...fixture.environment, ...environmentOverrides } });
}

async function cleanupReuseFixture(fixture) {
  await Promise.all([
    fs.rm(fixture.directory, { recursive: true, force: true }),
    fs.rm(fixture.commandStubs, { recursive: true, force: true }),
  ]);
}

function parseTeamId(identity) {
  return execFileAsync("bash", [
    "-c",
    'source "$1"; team_id_from_codesign_identity "$2"',
    "cadence-team-id-parser-test",
    buildScript,
    identity,
  ]);
}

async function runTagVerifierFixture(responses, tag, sourceSha, maxPeelDepth) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-tag-verifier-test-"));
  const fixturePath = path.join(root, "responses.json");
  const ghPath = path.join(root, "gh");
  await fs.writeFile(fixturePath, JSON.stringify(responses));
  await fs.writeFile(ghPath, `#!/usr/bin/env node
const fs = require("node:fs");

const args = process.argv.slice(2);
const endpoint = args.find((argument) => argument.startsWith("repos/"));
const responses = JSON.parse(fs.readFileSync(process.env.FAKE_GH_FIXTURE, "utf8"));
const response = endpoint ? responses[endpoint] : undefined;
if (!response) process.exit(1);
if (response.exit) process.exit(response.exit);
if (Object.hasOwn(response, "stdout")) {
  process.stdout.write(response.stdout);
  process.exit(0);
}
if (!response.object || typeof response.object.type !== "string" || typeof response.object.sha !== "string") process.exit(1);
process.stdout.write(\`\${response.object.type}\\t\${response.object.sha}\\n\`);
`);
  await fs.chmod(ghPath, 0o755);
  const env = {
    ...process.env,
    PATH: `${root}${path.delimiter}${path.dirname(process.execPath)}${path.delimiter}${process.env.PATH || ""}`,
    FAKE_GH_FIXTURE: fixturePath,
    GH_TOKEN: "fixture-token-that-must-not-be-printed",
  };
  const args = [
    tagVerifierScript,
    "--repository", "example/cadence",
    "--tag", tag,
    "--source-sha", sourceSha,
  ];
  if (maxPeelDepth !== undefined) args.push("--max-peel-depth", String(maxPeelDepth));
  try {
    return await execFileAsync(tagVerifierScript, args.slice(1), { env });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}

function validateOutputDirectory(requested, callerDirectory) {
  return execFileAsync("bash", [
    "-c",
    'source "$1"; validate_release_output_dir "$2" "$3"',
    "cadence-output-dir-validation-test",
    buildScript,
    requested,
    callerDirectory,
  ]);
}

function prepareOutputDirectory(requested, callerDirectory) {
  return execFileAsync("bash", [
    "-c",
    'source "$1"; prepare_release_output_dir "$2" "$3"',
    "cadence-output-dir-preparation-test",
    buildScript,
    requested,
    callerDirectory,
  ]);
}

function verifyScreenshotProvenance(releaseSourceSha, screenshotPath, metadataPath) {
  return execFileAsync("bash", [
    "-c",
    'source "$1"; verify_release_screenshot_provenance "$2" "$3" "$4"',
    "cadence-screenshot-provenance-test",
    buildScript,
    releaseSourceSha,
    screenshotPath,
    metadataPath,
  ]);
}

async function assertRejectedOutputDirectory(label, requested, callerDirectory) {
  await assert.rejects(
    validateOutputDirectory(requested, callerDirectory),
    (error) => error.code === 1 && error.stderr.includes("invalid release output directory"),
    `${label} must be rejected`,
  );
}

test("Team ID parser accepts a valid identity suffix and rejects malformed identities", async () => {
  const { stdout } = await parseTeamId("Developer ID Application: Cadence Release (ABCDE12345)");
  assert.equal(stdout.trim(), "ABCDE12345");

  for (const identity of [
    "Developer ID Application: Cadence Release",
    "Developer ID Application: Cadence Release (ABCDE1234)",
    "Developer ID Installer: Cadence Release (ABCDE12345)",
  ]) {
    await assert.rejects(
      parseTeamId(identity),
      (error) => error.code === 1 && error.stderr.includes("valid ten-character Team ID"),
    );
  }
});

test("release output directory helpers enforce safe, caller-relative targets", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-output-dir-test-"));
  try {
    const realRoot = await fs.realpath(root);
    const emptyDirectory = path.join(realRoot, "empty");
    const visibleNonemptyDirectory = path.join(realRoot, "visible-nonempty");
    const hiddenNonemptyDirectory = path.join(realRoot, "hidden-nonempty");
    const file = path.join(realRoot, "release-file");
    const symlink = path.join(realRoot, "release-symlink");
    const fifo = path.join(realRoot, "release-fifo");
    const spacedParent = path.join(realRoot, "parent with spaces");
    const missingDirectory = path.join(realRoot, "missing");
    const createdDirectory = path.join(realRoot, "created");
    const repositoryRoot = path.resolve(releaseDirectory, "../..");

    await fs.mkdir(emptyDirectory);
    await fs.mkdir(visibleNonemptyDirectory);
    await fs.writeFile(path.join(visibleNonemptyDirectory, "sentinel.txt"), "preserve me\n");
    await fs.mkdir(hiddenNonemptyDirectory);
    const hiddenSentinel = path.join(hiddenNonemptyDirectory, ".sentinel");
    await fs.writeFile(hiddenSentinel, "preserve hidden me\n");
    await fs.writeFile(file, "not a directory\n");
    await fs.symlink(emptyDirectory, symlink, "dir");
    await execFileAsync("mkfifo", [fifo]);
    await fs.mkdir(spacedParent);

    for (const [label, requested, expected] of [
      ["missing target", "missing", missingDirectory],
      ["empty target", "empty", emptyDirectory],
      [
        "spaces in a caller-relative path",
        "parent with spaces/release output",
        path.join(spacedParent, "release output"),
      ],
    ]) {
      const { stdout } = await validateOutputDirectory(requested, root);
      assert.equal(stdout.trim(), expected, `${label} should resolve from the caller cwd`);
    }
    await assert.rejects(fs.lstat(missingDirectory), { code: "ENOENT" });

    const { stdout: createdOutput } = await prepareOutputDirectory("created", root);
    assert.equal(createdOutput.trim(), createdDirectory);
    assert.deepEqual((await fs.readdir(createdDirectory)), []);

    for (const [label, requested] of [
      ["visible nonempty target", "visible-nonempty"],
      ["hidden nonempty target", "hidden-nonempty"],
      ["regular file target", "release-file"],
      ["symlink target", "release-symlink"],
      ["special node target", "release-fifo"],
      ["ambiguous dot target", "."],
      ["ambiguous parent target", ".."],
      ["root target", "/"],
      ["repository root target", repositoryRoot],
      ["repository root equivalent target", `${repositoryRoot}/scripts/../../${path.basename(repositoryRoot)}`],
      ["missing parent", "missing-parent/release output"],
    ]) {
      await assertRejectedOutputDirectory(label, requested, root);
    }
    await assert.rejects(
      execFileAsync(buildScript, ["--version", cargoPackageVersion, "--output-dir", visibleNonemptyDirectory]),
      (error) => error.code === 2
        && error.stderr.includes("target must be empty")
        && !error.stderr.includes("missing required production signing secret"),
    );
    assert.equal(await fs.readFile(path.join(visibleNonemptyDirectory, "sentinel.txt"), "utf8"), "preserve me\n");
    assert.equal(await fs.readFile(hiddenSentinel, "utf8"), "preserve hidden me\n");
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("release output cleanup keeps temporary cleanup but never removes caller output", async () => {
  const script = await fs.readFile(buildScript, "utf8");
  assert.doesNotMatch(script, /rm -rf "\$output_dir"/);
  assert.match(script, /rm -rf "\$work_dir"/);
  assert.ok(
    script.indexOf("resolve_verified_source_sha")
      < script.indexOf('validate_release_output_dir "$output_dir" "$caller_cwd"'),
  );
});

test("checked-in release screenshot provenance validates its recorded source commit", async () => {
  const { stdout: headOutput } = await execFileAsync("git", ["rev-parse", "HEAD"], { cwd: projectDirectory });
  const { stdout: sourceOutput } = await verifyScreenshotProvenance(
    headOutput.trim(),
    path.join(projectDirectory, "reference", "cadence-ui-repainted.png"),
    path.join(projectDirectory, "reference", "cadence-ui-repainted.png.json"),
  );
  assert.equal(sourceOutput.trim(), introducedScreenshotSourceGitSha);
});

test("release screenshot provenance fails closed when its metadata sidecar is absent", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-screenshot-provenance-test-"));
  try {
    const screenshotPath = path.join(root, "cadence-ui-repainted.png");
    const metadataPath = path.join(root, "cadence-ui-repainted.png.json");
    await fs.copyFile(path.join(projectDirectory, "reference", "cadence-ui-repainted.png"), screenshotPath);
    const { stdout: headOutput } = await execFileAsync("git", ["rev-parse", "HEAD"], { cwd: projectDirectory });

    await assert.rejects(
      verifyScreenshotProvenance(headOutput.trim(), screenshotPath, metadataPath),
      (error) => error.code === 1 && error.stderr.includes("metadata sidecar is missing"),
    );
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("release build rejects malformed and mismatched provenance before output or signing work", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-provenance-test-"));
  try {
    const outputDirectory = path.join(root, "release-output");
    const runBuild = (githubSha) => execFileAsync(
      buildScript,
      ["--version", "0.1.0", "--output-dir", outputDirectory],
      {
        cwd: projectDirectory,
        env: { ...process.env, GITHUB_SHA: githubSha },
      },
    );

    await assert.rejects(
      runBuild("not-a-commit-sha"),
      (error) => error.code === 1
        && error.stderr.includes("GITHUB_SHA must be a 40-character commit SHA")
        && !error.stderr.includes("missing required production signing secret")
        && !error.stderr.includes("Cadence production releases must be built on macOS"),
    );
    await assert.rejects(fs.lstat(outputDirectory), { code: "ENOENT" });

    const { stdout: headOutput } = await execFileAsync(
      "git",
      ["rev-parse", "HEAD"],
      { cwd: projectDirectory },
    );
    const headSha = headOutput.trim();
    const mismatchedSha = headSha === "a".repeat(40) ? "b".repeat(40) : "a".repeat(40);
    await assert.rejects(
      runBuild(mismatchedSha),
      (error) => error.code === 1
        && error.stderr.includes("GITHUB_SHA does not match the checked-out repository HEAD")
        && !error.stderr.includes("missing required production signing secret"),
    );
    await assert.rejects(fs.lstat(outputDirectory), { code: "ENOENT" });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("release version validation accepts every channel only at the locked package base", async () => {
  const validateVersion = (channel, requestedVersion) => execFileAsync("bash", [
    "-c",
    'source "$1"; channel="$2"; version="$3"; validate_release_version_against_cargo "$(resolve_root_cargo_package_version)" "$(resolve_locked_cargo_package_version)"',
    "cadence-version-validation-test",
    buildScript,
    channel,
    requestedVersion,
  ]);

  for (const [channel, requestedVersion] of [
    ["stable", cargoPackageVersion],
    ["rc", `${cargoPackageVersion}-rc.1`],
    ["nightly", `${cargoPackageVersion}-nightly.1`],
  ]) {
    await assert.doesNotReject(validateVersion(channel, requestedVersion));
  }
  await assert.rejects(
    validateVersion("stable", "0.1.8"),
    (error) => error.code === 1 && error.stderr.includes("does not match root cadence-native package version"),
  );
  await assert.rejects(
    validateVersion("nightly", "0.1.8-nightly.1"),
    (error) => error.code === 1 && error.stderr.includes("does not match root cadence-native package base version"),
  );
  await assert.rejects(
    execFileAsync("bash", [
      "-c",
      'source "$1"; channel=stable; version="$2"; validate_release_version_against_cargo "$3" "$4"',
      "cadence-lock-version-validation-test",
      buildScript,
      cargoPackageVersion,
      cargoPackageVersion,
      "0.1.8",
    ]),
    (error) => error.code === 1 && error.stderr.includes("Cargo.toml and Cargo.lock cadence-native package versions differ"),
  );
});

test("direct release builder rejects a package-version mismatch before platform, credentials, or output work", async () => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-version-mismatch-test-"));
  try {
    const outputDirectory = path.join(root, "release-output");
    const { stdout: headOutput } = await execFileAsync("git", ["rev-parse", "HEAD"], { cwd: projectDirectory });
    await assert.rejects(
      execFileAsync(
        buildScript,
        ["--version", "0.1.8", "--channel", "stable", "--output-dir", outputDirectory],
        { cwd: projectDirectory, env: { ...process.env, GITHUB_SHA: headOutput.trim() } },
      ),
      (error) => error.code === 1
        && error.stderr.includes("does not match root cadence-native package version")
        && !error.stderr.includes("Cadence production releases must be built on macOS")
        && !error.stderr.includes("missing required production signing secret")
        && !error.stderr.includes("invalid release output directory"),
    );
    await assert.rejects(fs.lstat(outputDirectory), { code: "ENOENT" });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
});

test("tag target verifier resolves lightweight and annotated tags and rejects unsafe targets", async (t) => {
  const sourceSha = "b".repeat(40);
  const lightweightTag = "v0.1.9";
  const annotatedTag = "v0.1.9-annotated";
  const annotatedObjectSha = "c".repeat(40);
  const endpoint = (suffix) => `repos/example/cadence/${suffix}`;

  await t.test("lightweight tag match", async () => {
    const { stdout } = await runTagVerifierFixture(
      { [endpoint(`git/ref/tags/${lightweightTag}`)]: { object: { type: "commit", sha: sourceSha } } },
      lightweightTag,
      sourceSha,
    );
    assert.match(stdout, new RegExp(`${lightweightTag} -> ${sourceSha}`));
  });

  await t.test("annotated tag match", async () => {
    const { stdout } = await runTagVerifierFixture(
      {
        [endpoint(`git/ref/tags/${annotatedTag}`)]: { object: { type: "tag", sha: annotatedObjectSha } },
        [endpoint(`git/tags/${annotatedObjectSha}`)]: { object: { type: "commit", sha: sourceSha.toUpperCase() } },
      },
      annotatedTag,
      sourceSha,
    );
    assert.match(stdout, new RegExp(`${annotatedTag} -> ${sourceSha}`));
  });

  await t.test("mismatched commit", async () => {
    const mismatchedSha = "d".repeat(40);
    await assert.rejects(
      runTagVerifierFixture(
        { [endpoint(`git/ref/tags/${lightweightTag}`)]: { object: { type: "commit", sha: mismatchedSha } } },
        lightweightTag,
        sourceSha,
      ),
      (error) => error.code === 1
        && error.stderr.includes("does not resolve to SOURCE_SHA")
        && !error.stderr.includes("fixture-token-that-must-not-be-printed"),
    );
  });

  await t.test("malformed target", async () => {
    await assert.rejects(
      runTagVerifierFixture(
        { [endpoint(`git/ref/tags/${lightweightTag}`)]: { stdout: "tag\\tnot-a-sha\\n" } },
        lightweightTag,
        sourceSha,
      ),
      (error) => error.code === 1 && error.stderr.includes("tag target API response is malformed"),
    );
  });

  await t.test("annotated target cycle", async () => {
    const cycleSha = "e".repeat(40);
    await assert.rejects(
      runTagVerifierFixture(
        {
          [endpoint(`git/ref/tags/${annotatedTag}`)]: { object: { type: "tag", sha: cycleSha } },
          [endpoint(`git/tags/${cycleSha}`)]: { object: { type: "tag", sha: cycleSha } },
        },
        annotatedTag,
        sourceSha,
      ),
      (error) => error.code === 1 && error.stderr.includes("cycle detected"),
    );
  });

  await t.test("excessive annotated target peel", async () => {
    const peelShas = ["f", "1", "2", "3"].map((digit) => digit.repeat(40));
    const responses = {
      [endpoint(`git/ref/tags/${annotatedTag}`)]: { object: { type: "tag", sha: peelShas[0] } },
    };
    for (let index = 0; index < peelShas.length - 1; index += 1) {
      responses[endpoint(`git/tags/${peelShas[index]}`)] = {
        object: { type: "tag", sha: peelShas[index + 1] },
      };
    }
    responses[endpoint(`git/tags/${peelShas.at(-1)}`)] = { object: { type: "commit", sha: sourceSha } };
    await assert.rejects(
      runTagVerifierFixture(responses, annotatedTag, sourceSha, 2),
      (error) => error.code === 1 && error.stderr.includes("peel depth exceeds"),
    );
  });
});

test("release build uses verified HEAD and isolates Apple credentials from Cargo and bundle helpers", async () => {
  const script = await fs.readFile(buildScript, "utf8");
  const provenanceIndex = script.indexOf("if ! source_git_sha=\"$(resolve_verified_source_sha)\"; then");
  const metadataIndex = script.indexOf("if ! root_cargo_package_version=\"$(resolve_root_cargo_package_version)\"; then");
  const lockVersionIndex = script.indexOf("if ! locked_cargo_package_version=\"$(resolve_locked_cargo_package_version)\"; then");
  const packageValidationIndex = script.indexOf("validate_release_version_against_cargo \"$root_cargo_package_version\"");
  const outputValidationIndex = script.indexOf('if ! output_dir="$(validate_release_output_dir "$output_dir" "$caller_cwd")"; then');
  const platformCheckIndex = script.indexOf('if [[ "$(uname -s)" != "Darwin" ]]; then');
  const cargoIndex = script.indexOf("cargo build --target");
  const decodeIndex = script.indexOf('decode_base64 "$apple_developer_id_application_cert_base64"');
  const keychainImportIndex = script.indexOf('security import "$certificate_path"');
  const outputPreparationIndex = script.indexOf('if ! output_dir="$(prepare_release_output_dir "$output_dir" "$caller_cwd")"; then');
  const cleanCheckoutIndex = script.indexOf('if [[ -n "$(git -C "$project_dir" status --porcelain --untracked-files=all)" ]]; then');
  const screenshotVerificationIndex = script.indexOf('if ! screenshot_source_git_sha="$(verify_release_screenshot_provenance "$source_git_sha")"; then');
  assert.ok(provenanceIndex >= 0, "the build must resolve a verified source SHA");
  assert.ok(metadataIndex > provenanceIndex, "package metadata must follow source provenance");
  assert.ok(lockVersionIndex > metadataIndex, "the lockfile version must be checked after metadata");
  assert.ok(packageValidationIndex > lockVersionIndex, "the requested version must be checked against Cargo versions");
  assert.ok(outputValidationIndex > packageValidationIndex, "version validation must precede output validation");
  assert.ok(platformCheckIndex > packageValidationIndex, "version validation must precede the macOS platform check");
  assert.ok(outputValidationIndex > provenanceIndex, "provenance must precede output validation");
  assert.ok(cargoIndex > provenanceIndex, "Cargo must use verified provenance");
  assert.ok(decodeIndex > cargoIndex, "Cargo must precede certificate decoding");
  assert.ok(keychainImportIndex > cargoIndex, "Cargo must precede keychain import");
  assert.ok(outputPreparationIndex > cargoIndex, "Cargo must precede release output creation");
  assert.ok(cleanCheckoutIndex >= 0, "the build must require a clean checkout");
  assert.ok(screenshotVerificationIndex > cleanCheckoutIndex, "screenshot provenance must use a clean checkout");
  assert.ok(cargoIndex > screenshotVerificationIndex, "screenshot provenance must precede the signed build");
  assert.match(script, /head_sha=.*rev-parse --verify HEAD\^\{commit\}/);
  assert.match(script, /provided_sha=.*GITHUB_SHA/);
  assert.match(script, /tr '\[:upper:\]' '\[:lower:\]'/);
  assert.match(script, /GITHUB_SHA does not match the checked-out repository HEAD/);
  assert.match(script, /--git-sha "\$source_git_sha"/);
  assert.match(script, /--screenshot-source-git-sha "\$screenshot_source_git_sha"/);
  assert.match(script, /reference\/cadence-ui-repainted\.png\.json/);
  assert.match(script, /shasum -a 256 "\$screenshot_path"/);
  assert.match(script, /sips -g pixelWidth -g pixelHeight/);
  assert.match(script, /merge-base --is-ancestor/);
  assert.match(script, /cp "\$project_dir\/reference\/cadence-ui-repainted\.png"/);
  assert.match(script, /apple_developer_id_application_cert_base64="\$\{APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64:-\}"/);
  assert.match(script, /apple_developer_id_application_cert_password="\$\{APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD:-\}"/);
  assert.match(script, /unset \\\n(?:.*\\\n){5}/);
  assert.match(script, /env \\\n(?:        -u APPLE_[^\n]+ \\\n){6}        cargo metadata/);
  assert.equal((script.match(/CADENCE_DISTRIBUTION_BUILD=1/g) ?? []).length, 1);
  assert.match(script, /env \\\n(?:    -u APPLE_[^\n]+ \\\n){6}    CADENCE_DISTRIBUTION_BUILD=1 \\\n    cargo build --target/);
  assert.match(script, /--executable "\$executable_path"/);
});

test("manifest preserves the stable default and emits supported channels", async (t) => {
  for (const [label, version, channel] of [
    ["stable default", "0.1.0", undefined],
    ["rc", "0.1.0-rc.1", "rc"],
    ["nightly", "0.1.0-nightly.1", "nightly"],
  ]) {
    await t.test(label, async () => {
      const directory = await createInputDirectory(version);
      try {
        const manifest = await createManifest(directory, version, channel);
        assert.equal(manifest.channel, channel || "stable");
        assert.equal(manifest.version, version);
        assert.equal(manifest.source.git_sha, gitSha);
        assert.equal(manifest.screenshot.source_git_sha, screenshotSourceGitSha);
        assert.notEqual(manifest.source.git_sha, manifest.screenshot.source_git_sha);
      } finally {
        await fs.rm(directory, { recursive: true, force: true });
      }
    });
  }
});

test("manifest rejects unknown channels and mismatched channel versions", async () => {
  for (const [channel, version] of [["canary", "0.1.0"], ["nightly", "0.1.0"], ["rc", "0.1.0-nightly.1"]]) {
    const directory = await createInputDirectory(version);
    try {
      await assert.rejects(
        execFileAsync(process.execPath, [
          manifestScript,
          "--output-dir", directory,
          "--version", version,
          "--channel", channel,
          "--build-id", "cadence-test-build",
          "--git-sha", gitSha,
          "--screenshot-source-git-sha", screenshotSourceGitSha,
          "--released-at", "2026-08-09T00:00:00Z",
          "--team-id", "TEAM123456",
          "--notary-submission-id", notarySubmissionId,
        ]),
        (error) => error.code === 2,
      );
    } finally {
      await fs.rm(directory, { recursive: true, force: true });
    }
  }
});

test("manifest creation rejects a missing or empty output directory before artifact work", async (t) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-manifest-output-dir-test-"));
  const commonArgs = [
    "--version", "0.1.0",
    "--build-id", "cadence-test-build",
    "--git-sha", gitSha,
    "--screenshot-source-git-sha", screenshotSourceGitSha,
    "--released-at", "2026-08-09T00:00:00Z",
    "--team-id", "TEAM123456",
    "--notary-submission-id", notarySubmissionId,
  ];
  try {
    for (const [label, outputArgs] of [["missing", []], ["empty", ["--output-dir", ""]]]) {
      await t.test(label, async () => {
        await assert.rejects(
          execFileAsync(process.execPath, [manifestScript, ...outputArgs, ...commonArgs], { cwd: directory }),
          (error) => error.code === 2 && error.stderr.includes("output-dir is required"),
        );
        await assert.rejects(fs.lstat(path.join(directory, "cadence-release-manifest.json")), { code: "ENOENT" });
      });
    }
  } finally {
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release publisher streams exact artifacts and commits only after ordered validation", async () => {
  const directory = await createInputDirectory("0.1.0");
  const manifest = await createManifest(directory, "0.1.0");
  const server = await startReleaseServer();
  try {
    const result = await publishRelease(directory, server.endpoint);
    assert.match(result.stdout, /"committed": true/);
    const manifestBytes = await fs.readFile(path.join(directory, "cadence-release-manifest.json"));
    const descriptors = [manifest.artifacts[0], manifest.screenshot, manifest.changelog];
    assert.equal(server.requests.length, 5, "capability, three files, then commit");
    assert.equal(server.requests[0].method, "GET");
    for (const [index, descriptor] of descriptors.entries()) {
      const request = server.requests[index + 1];
      assert.equal(request.method, "PUT");
      assert.match(request.url, new RegExp(`/staging/files/${encodeURIComponent(descriptor.name)}$`));
      assert.equal(request.headers.authorization, "Bearer fixture-release-token");
      assert.equal(request.headers["content-type"], "application/octet-stream");
      assert.equal(Number(request.headers["content-length"]), descriptor.size_bytes);
      assert.equal(request.body.length, descriptor.size_bytes);
      assert.equal(
        request.headers["x-portalsurfer-sha256"],
        crypto.createHash("sha256").update(request.body).digest("hex"),
      );
      assert.equal(request.headers["x-portalsurfer-release-version"], manifest.version);
      assert.equal(request.headers["x-portalsurfer-release-channel"], manifest.channel);
      assert.equal(request.headers["x-portalsurfer-released-at"], manifest.released_at);
    }
    const commit = server.requests.at(-1);
    assert.equal(commit.method, "PUT");
    assert.match(commit.url, /\/commit$/);
    assert.equal(commit.headers.authorization, "Bearer fixture-release-token");
    assert.equal(commit.headers["content-type"], "application/vnd.portalsurfer.release-manifest+json;version=2");
    assert.equal(Number(commit.headers["content-length"]), manifestBytes.length);
    assert.deepEqual(commit.body, manifestBytes);
    assert.equal(
      commit.headers["x-portalsurfer-manifest-sha256"],
      crypto.createHash("sha256").update(manifestBytes).digest("hex"),
    );

    const publisher = await fs.readFile(publisherScript, "utf8");
    assert.doesNotMatch(publisher, /fs\.readFile\(filePath/);
    assert.match(publisher, /createReadStream\(\{ autoClose: true, start: 0 \}\)/);
    assert.match(publisher, /duplex: "half"/);
  } finally {
    await server.close();
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release publisher rejects noncanonical production origins before network activity", async () => {
  const directory = await createInputDirectory("0.1.0");
  await createManifest(directory, "0.1.0");
  const guard = await createFetchGuard();
  try {
    for (const endpoint of [
      "https://portalsurfer.org:444",
      "https://portalsurfer.org:443",
      "https://portalsurfer.org/releases",
      "https://portalsurfer.org/",
      "https://portalsurfer.org/..",
      "https://portalsurfer.org/%2e%2e",
      "https://release-user:release-password@portalsurfer.org",
      "https://portalsurfer.org/?test=1",
      "https://portalsurfer.org/#test",
      " https://portalsurfer.org",
      "https://portalsurfer.org ",
    ]) {
      await assert.rejects(
        publishRelease(directory, endpoint, { env: guard.environment }),
        (error) => (error.code === 1 || error.code === 2)
          && error.stderr.includes("endpoint must be"),
        `endpoint ${endpoint} must be rejected before fetch`,
      );
    }
  } finally {
    await guard.cleanup();
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release publisher rejects a symlink-backed manifest before network activity", async () => {
  const directory = await createInputDirectory("0.1.0");
  await createManifest(directory, "0.1.0");
  const manifestPath = path.join(directory, "cadence-release-manifest.json");
  const manifestTarget = path.join(directory, "manifest-target.json");
  await fs.rename(manifestPath, manifestTarget);
  await fs.symlink(manifestTarget, manifestPath);
  const server = await startReleaseServer();
  try {
    await assert.rejects(
      publishRelease(directory, server.endpoint),
      (error) => (error.code === 1 || error.code === 2)
        && error.stderr.includes("manifest")
        && error.stderr.includes("regular file"),
    );
    assert.equal(server.requests.length, 0);
  } finally {
    await server.close();
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release publisher rejects a symlink-backed descriptor before its upload or commit", async () => {
  const directory = await createInputDirectory("0.1.0");
  const manifest = await createManifest(directory, "0.1.0");
  const descriptor = manifest.screenshot;
  const descriptorPath = path.join(directory, descriptor.name);
  const descriptorTarget = path.join(directory, "screenshot-target.png");
  await fs.rename(descriptorPath, descriptorTarget);
  await fs.symlink(descriptorTarget, descriptorPath);
  const server = await startReleaseServer();
  try {
    await assert.rejects(
      publishRelease(directory, server.endpoint),
      (error) => error.code === 1
        && error.stderr.includes(`artifact ${descriptor.name}`)
        && error.stderr.includes("regular file"),
    );
    const uploads = server.requests.filter((request) => request.method === "PUT");
    assert.equal(uploads.length, 1, "only the descriptor before the symlink may upload");
    assert.match(uploads[0].url, new RegExp(`/staging/files/${encodeURIComponent(manifest.artifacts[0].name)}$`));
    assert.equal(server.requests.some((request) => request.url?.endsWith("/commit")), false);
  } finally {
    await server.close();
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release publisher stops before commit on staging failure or metadata mismatch", async (t) => {
  await t.test("staging failure", async () => {
    const directory = await createInputDirectory("0.1.0");
    const manifest = await createManifest(directory, "0.1.0");
    const server = await startReleaseServer({ failStageName: manifest.screenshot.name });
    try {
      await assert.rejects(
        publishRelease(directory, server.endpoint),
        (error) => error.code === 1 && error.stderr.includes(`staging ${manifest.screenshot.name} failed`),
      );
      assert.equal(server.requests.some((request) => request.url?.endsWith("/commit")), false);
    } finally {
      await server.close();
      await fs.rm(directory, { recursive: true, force: true });
    }
  });

  await t.test("artifact hash mismatch", async () => {
    const directory = await createInputDirectory("0.1.0");
    await createManifest(directory, "0.1.0");
    const manifestPath = path.join(directory, "cadence-release-manifest.json");
    const manifest = JSON.parse(await fs.readFile(manifestPath, "utf8"));
    const artifactPath = path.join(directory, manifest.artifacts[0].name);
    const artifact = await fs.readFile(artifactPath);
    artifact[0] ^= 0xff;
    await fs.writeFile(artifactPath, artifact);
    const server = await startReleaseServer();
    try {
      await assert.rejects(
        publishRelease(directory, server.endpoint),
        (error) => error.code === 1 && error.stderr.includes(`manifest metadata does not match ${manifest.artifacts[0].name}`),
      );
      assert.equal(server.requests.filter((request) => request.method === "PUT").length, 0);
    } finally {
      await server.close();
      await fs.rm(directory, { recursive: true, force: true });
    }
  });
});

test("release publisher rejects a missing artifact before commit", async () => {
  const directory = await createInputDirectory("0.1.0");
  const manifest = await createManifest(directory, "0.1.0");
  await fs.rm(path.join(directory, manifest.artifacts[0].name));
  const server = await startReleaseServer();
  try {
    await assert.rejects(
      publishRelease(directory, server.endpoint),
      (error) => error.code === "ENOENT" || error.code === 1,
    );
    assert.equal(server.requests.some((request) => request.url?.endsWith("/commit")), false);
  } finally {
    await server.close();
    await fs.rm(directory, { recursive: true, force: true });
  }
});

test("release reuse verifier accepts a complete production candidate", async () => {
  const fixture = await createReuseFixture();
  try {
    const result = await verifyReuseFixture(fixture);
    assert.match(result.stdout, /"verified": true/);
    assert.match(result.stdout, /"build_id": "cadence-test-build"/);
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("manifest creation and reuse verification stream the ZIP without buffered reads", async () => {
  const version = "0.1.0";
  const directory = await createInputDirectory(version);
  const artifactPath = path.join(directory, `cadence-v${version}-macos-arm64.zip`);
  const guard = await createBufferedReadGuard(artifactPath);
  let commandStubs;
  try {
    await assert.rejects(
      forceBufferedRead(artifactPath, guard.environment),
      (error) => error.code === 1 && error.stderr.includes("buffered read guard rejected"),
      "the guard must catch a forced buffered ZIP read",
    );

    const manifest = await createManifest(directory, version, undefined, { env: guard.environment });
    await writeReuseSums(directory, manifest);
    commandStubs = await createReuseCommandStubs();
    const environment = {
      ...guard.environment,
      PATH: `${commandStubs}${path.delimiter}${guard.environment.PATH || ""}`,
    };
    const fixture = { directory, manifest, version, commandStubs, environment };
    const result = await verifyReuseFixture(fixture);
    assert.match(result.stdout, /"verified": true/);
  } finally {
    await fs.rm(directory, { recursive: true, force: true });
    if (commandStubs) await fs.rm(commandStubs, { recursive: true, force: true });
    await guard.cleanup();
  }
});

test("release reuse verifier rejects a missing required file", async () => {
  const fixture = await createReuseFixture();
  try {
    await fs.rm(path.join(fixture.directory, fixture.manifest.artifacts[0].name));
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("required file is missing"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects malformed manifest JSON", async () => {
  const fixture = await createReuseFixture();
  try {
    await fs.writeFile(path.join(fixture.directory, "cadence-release-manifest.json"), "{not-json\n");
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("manifest is not valid JSON"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects provenance mismatches", async () => {
  const fixture = await createReuseFixture();
  try {
    fixture.manifest.source.repository = "example/other-cadence";
    await fs.writeFile(
      path.join(fixture.directory, "cadence-release-manifest.json"),
      `${JSON.stringify(fixture.manifest, null, 2)}\n`,
    );
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("source repository does not match"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects descriptor hash mismatches", async () => {
  const fixture = await createReuseFixture();
  try {
    const artifactPath = path.join(fixture.directory, fixture.manifest.artifacts[0].name);
    const artifact = await fs.readFile(artifactPath);
    artifact[0] ^= 0xff;
    await fs.writeFile(artifactPath, artifact);
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("SHA-256 does not match"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects non-ZIP artifact bytes with self-consistent metadata", async () => {
  const fixture = await createReuseFixture();
  try {
    await refreshReuseArtifactDescriptor(fixture, Buffer.from("not a ZIP archive\n"));
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("artifact archive is not a valid ZIP"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects a truncated ZIP with self-consistent metadata", async () => {
  const fixture = await createReuseFixture();
  try {
    const archive = await fs.readFile(path.join(fixture.directory, fixture.manifest.artifacts[0].name));
    await refreshReuseArtifactDescriptor(fixture, archive.subarray(0, Math.max(1, Math.floor(archive.length / 2))));
    await assert.rejects(
      verifyReuseFixture(fixture),
      (error) => error.code === 1 && error.stderr.includes("artifact archive is not a valid ZIP"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("release reuse verifier rejects signature verification failure", async () => {
  const fixture = await createReuseFixture();
  try {
    await assert.rejects(
      verifyReuseFixture(fixture, { env: { CADENCE_TEST_CODESIGN_FAILURE: "1" } }),
      (error) => error.code === 1 && error.stderr.includes("codesign verification failed"),
    );
  } finally {
    await cleanupReuseFixture(fixture);
  }
});

test("nightly allocator advances the initial, consecutive, and cross-channel patch stream", async (t) => {
  await t.test("initial release", async () => {
    assert.equal(await allocateNightlyVersion("0.1.0", ""), "0.1.1");
  });
  await t.test("consecutive release", async () => {
    assert.equal(
      await allocateNightlyVersion("0.1.1", "Cadence 0.1.1-nightly.6 nightly\n"),
      "0.1.2",
    );
  });
  await t.test("stable, rc, and nightly share one high-water mark", async () => {
    assert.equal(
      await allocateNightlyVersion(
        "0.1.2",
        "Cadence 0.1.0\nCadence 0.1.1-rc.2 rc\nCadence 0.1.2-nightly.7 nightly\n",
      ),
      "0.1.3",
    );
  });
});

test("nightly allocator reuses a one-ahead Cargo reservation", async () => {
  assert.equal(
    await allocateNightlyVersion("0.1.2", "Cadence 0.1.1-nightly.8 nightly\n"),
    "0.1.2",
  );
});

test("nightly allocator fails closed for history drift, gaps, and malformed names", async (t) => {
  for (const [label, cargoVersion, releaseNames, message] of [
    ["Cargo behind", "0.1.0", "Cadence 0.1.1-nightly.1 nightly\n", "behind published release history"],
    ["Cargo more than one patch ahead", "0.1.4", "Cadence 0.1.2-nightly.1 nightly\n", "patch steps"],
    ["Cargo major-minor mismatch", "0.2.0", "Cadence 0.1.2\n", "not in the published patch-version stream"],
    ["malformed release", "0.1.0", "Cadence 0.1.0-nightly.1\n", "Malformed published Cadence release name"],
    ["empty release name", "0.1.0", "Cadence 0.1.0\n\n", "empty name"],
  ]) {
    await t.test(label, async () => {
      await assert.rejects(
        allocateNightlyVersion(cargoVersion, releaseNames),
        (error) => error.code === 1 && error.stderr.includes(message),
      );
    });
  }
});

function workflowRunBlock(workflow, stepName) {
  const stepMarker = `      - name: ${stepName}\n`;
  const stepStart = workflow.indexOf(stepMarker);
  assert.ok(stepStart >= 0, `workflow must define the ${stepName} step`);

  const runMarker = "        run: |\n";
  const runStart = workflow.indexOf(runMarker, stepStart);
  assert.ok(runStart >= 0, `${stepName} must define a shell run block`);

  const bodyStart = runStart + runMarker.length;
  const nextStep = workflow.indexOf("\n      - name: ", bodyStart);
  const body = workflow.slice(bodyStart, nextStep >= 0 ? nextStep : workflow.length);
  return body
    .split("\n")
    .map((line) => (line.startsWith("          ") ? line.slice(10) : line))
    .join("\n")
    .trim();
}

test("release workflow rejects untrusted manual stable sources", async () => {
  const workflow = await fs.readFile(workflowPath, "utf8");
  const stableSourceCheck = workflowRunBlock(workflow, "Validate manual stable source");
  const runSourceCheck = (ref, version = "0.1.0") => execFileAsync(
    "bash",
    ["-c", stableSourceCheck],
    {
      env: {
        ...process.env,
        EVENT_NAME: "workflow_dispatch",
        REF: ref,
        INPUT_CHANNEL: "stable",
        INPUT_VERSION: version,
      },
    },
  );

  await assert.rejects(
    runSourceCheck("refs/heads/feature/release-test"),
    (error) => error.code === 1 && error.stderr.includes("manual stable releases must run from"),
  );
  await assert.doesNotReject(runSourceCheck("refs/heads/main"));
  await assert.doesNotReject(runSourceCheck("refs/tags/v0.1.0"));
  await assert.rejects(runSourceCheck("refs/tags/v0.1.1"), (error) => error.code === 1);
});

test("release workflow reserves nightly versions before immutable builds", async () => {
  const workflow = await fs.readFile(workflowPath, "utf8");
  const actionReferences = [...workflow.matchAll(
    /^[ \t]*uses:[ \t]+((?!\.\/|docker:\/\/)[^@\s]+\/[^@\s]+)@([^\s#]*)/gm,
  )];
  assert.ok(actionReferences.length > 0, "release workflow must define external repository action references");
  assert.doesNotMatch(
    workflow,
    /^[ \t]*uses:[ \t]+(?!\.\/|docker:\/\/)[^@\s]+\/[^@\s]+@v\d+(?:[ \t]|$)/m,
    "release workflow must not use mutable major-version external repository action references",
  );
  for (const [, action, reference] of actionReferences) {
    assert.match(reference, /^[0-9a-f]{40}$/, `${action} must use a full 40-hex commit SHA`);
  }
  const reservationMatch = workflow.match(/\n  reserve_nightly:\n([\s\S]*?)\n  prepare:\n/);
  const prepareMatch = workflow.match(/\n  prepare:\n([\s\S]*?)\n  build:\n/);
  const buildMatch = workflow.match(/\n  build:\n([\s\S]*?)\n  publish:\n/);
  assert.ok(reservationMatch, "workflow must define a nightly reservation job");
  assert.ok(prepareMatch, "workflow must define a preparation job");
  assert.ok(buildMatch, "workflow must define build and publish jobs");
  const reservationJob = reservationMatch[1];
  const prepareJob = prepareMatch[1];
  const buildJob = buildMatch[1];
  const publishJob = workflow.slice(buildMatch.index + buildMatch[0].length - "  publish:\n".length);

  assert.match(reservationJob, /runs-on: ubuntu-latest/);
  assert.match(reservationJob, /environment: cadence-production/);
  assert.match(reservationJob, /\n    if: \$\{\{ \(github\.event_name == 'schedule' \|\| \(github\.event_name == 'workflow_dispatch' && inputs\.channel == 'nightly'\)\) && github\.ref == 'refs\/heads\/main' \}\}\n/);
  assert.match(reservationJob, /\n    permissions:\n      contents: write\n/);
  assert.doesNotMatch(reservationJob, /github\.event_name == 'push'/);
  assert.match(reservationJob, /ref: \$\{\{ github\.sha \}\}/);
  assert.match(reservationJob, /gh api --paginate/);
  assert.match(reservationJob, /select\(\.draft == false and \.published_at != null\)/);
  assert.match(reservationJob, /allocate_nightly_version\.sh/);
  assert.match(reservationJob, /original_lock_version/);
  assert.match(reservationJob, /Cargo\.toml and Cargo\.lock Cadence package versions differ/);
  assert.match(reservationJob, /cadence-nightly-\$\{RUN_NUMBER\}-\$\{original_source_sha:0:12\}/);
  assert.match(reservationJob, /refs\/heads\/main/);
  assert.match(reservationJob, /write_reserved_package_version/);
  assert.match(reservationJob, /git push --atomic origin HEAD:refs\/heads\/main "refs\/tags\/\$reservation_tag"/);
  assert.match(reservationJob, /git add Cargo\.toml Cargo\.lock/);
  assert.match(reservationJob, /git diff --cached --name-only/);
  assert.match(reservationJob, /if \[\[ "\$source_sha" != "\$original_source_sha" \]\]; then/);
  assert.match(reservationJob, /assert_reservation_commit "\$source_sha" "\$reserved_cargo_version"/);
  assert.match(reservationJob, /git rev-list --parents -n 1 "\$commit"/);
  assert.match(reservationJob, /"\$commit \$original_source_sha"/);
  assert.match(reservationJob, /git diff-tree --no-commit-id --name-status -r "\$commit"/);
  assert.match(reservationJob, /reservation commit changed non-version content/);

  assert.match(prepareJob, /needs: reserve_nightly/);
  assert.match(prepareJob, /if: \$\{\{ always\(\) && \(needs\.reserve_nightly\.result == 'success' \|\| needs\.reserve_nightly\.result == 'skipped'\) \}\}/);
  assert.match(prepareJob, /\n    permissions:\n      contents: read\n/);
  assert.doesNotMatch(prepareJob, /contents: write/);
  assert.doesNotMatch(
    prepareJob,
    /git (add|commit|tag|push)|write_reserved_package_version|gh api --paginate|allocate_nightly_version\.sh/,
    "metadata preparation must not mutate the repository or reserve a nightly version",
  );
  assert.match(prepareJob, /ref: \$\{\{ needs\.reserve_nightly\.outputs\.source_sha \|\| github\.sha \}\}/);
  assert.match(prepareJob, /RESERVED_SOURCE_SHA: \$\{\{ needs\.reserve_nightly\.outputs\.source_sha \|\| '' \}\}/);
  assert.match(prepareJob, /RESERVED_BASE_VERSION: \$\{\{ needs\.reserve_nightly\.outputs\.reserved_base_version \|\| '' \}\}/);
  assert.match(prepareJob, /RESERVATION_TAG: \$\{\{ needs\.reserve_nightly\.outputs\.reservation_tag \|\| '' \}\}/);
  assert.match(prepareJob, /reserved_base_version="\$RESERVED_BASE_VERSION"/);
  assert.match(prepareJob, /reservation_tag="\$RESERVATION_TAG"/);
  assert.match(prepareJob, /refs\/heads\/main/);
  assert.match(prepareJob, /Cargo\.toml and Cargo\.lock Cadence package versions differ/);
  for (const output of ["channel", "version", "build_id", "release_tag", "tag_release", "create_github_release", "artifact_upload_name", "source_sha"]) {
    assert.match(prepareJob, new RegExp(`${output}: \\$\\{\\{ steps\\.release\\.outputs\\.${output} \\}\\}`));
    assert.match(buildJob, new RegExp(`${output}: \\$\\{\\{ needs\\.prepare\\.outputs\\.${output} \\}\\}`));
  }
  assert.match(buildJob, /needs: prepare/);
  assert.match(buildJob, /runs-on: macos-14/);
  assert.match(buildJob, /environment: cadence-production/);
  assert.match(buildJob, /\n    permissions:\n      contents: read\n      actions: read\n/);
  assert.match(buildJob, /- name: Check out source\n        uses: actions\/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\.4\.0\n        with:\n          ref: \$\{\{ needs\.prepare\.outputs\.source_sha \}\}\n          fetch-depth: 0\n          persist-credentials: false/);
  assert.doesNotMatch(buildJob, /Select release metadata|github\.event_name|steps\.release/);
  assert.match(buildJob, /GITHUB_SHA: \$\{\{ needs\.prepare\.outputs\.source_sha \}\}/);
  assert.match(buildJob, /export GITHUB_SHA=/);
  assert.match(buildJob, /actions\/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4\.6\.2/);
  assert.match(buildJob, /path: release-output\//);
  const jobEnvStart = buildJob.indexOf("    env:\n");
  const stepsStart = buildJob.indexOf("    steps:\n");
  assert.ok(jobEnvStart >= 0 && stepsStart > jobEnvStart, "build job must have a distinct job-level env block");
  const jobEnv = buildJob.slice(jobEnvStart, stepsStart);
  const signingStepStart = buildJob.indexOf("      - name: Build, sign, notarize, and describe release");
  const signingStepEnd = buildJob.indexOf("\n      - name: Upload immutable release artifact", signingStepStart);
  assert.ok(signingStepStart >= 0 && signingStepEnd > signingStepStart, "build job must have a signing step");
  const signingStep = buildJob.slice(signingStepStart, signingStepEnd);
  assert.doesNotMatch(buildJob, /- name: Verify Apple production secrets/);
  for (const secret of [
    "CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64",
    "CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD",
    "CADENCE_PRODUCTION_APPLE_NOTARY_KEY_BASE64",
    "CADENCE_PRODUCTION_APPLE_NOTARY_KEY_ID",
    "CADENCE_PRODUCTION_APPLE_NOTARY_ISSUER_ID",
  ]) {
    const secretReference = new RegExp(`\\$\\{\\{ secrets\\.${secret} \\}\\}`);
    assert.doesNotMatch(jobEnv, secretReference, `${secret} must not be job-scoped`);
    assert.match(signingStep, secretReference, `${secret} must be scoped to signing`);
    assert.equal(buildJob.match(new RegExp(secretReference.source, "g"))?.length, 1);
  }
  assert.doesNotMatch(buildJob, /CADENCE_RELEASE_UPLOAD_TOKEN|PORTALSURFER_RELEASE_ENDPOINT|publish_release\.mjs|gh release/);

  const reuseIndex = buildJob.indexOf("Download release reuse candidate");
  const verifyReuseIndex = buildJob.indexOf("Verify release reuse candidate");
  const signingIndex = buildJob.indexOf("Build, sign, notarize, and describe release");
  const checkoutIndex = buildJob.indexOf("Check out source");
  assert.ok(checkoutIndex >= 0 && checkoutIndex < reuseIndex && reuseIndex < verifyReuseIndex && verifyReuseIndex < signingIndex, "reuse must be downloaded after checkout and verified before signing");
  const reuseBlock = buildJob.slice(reuseIndex, signingIndex);
  assert.match(reuseBlock, /id: download_reuse_artifact/);
  assert.match(reuseBlock, /continue-on-error: true/);
  assert.match(reuseBlock, /actions\/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4\.3\.0/);
  assert.match(reuseBlock, /name: \$\{\{ needs\.prepare\.outputs\.artifact_upload_name \}\}/);
  assert.match(reuseBlock, /path: release-candidate\n/);
  assert.match(reuseBlock, /github-token: \$\{\{ github\.token \}\}/);
  assert.match(reuseBlock, /run-id: \$\{\{ github\.run_id \}\}/);
  assert.match(reuseBlock, /id: verify_reuse_artifact/);
  assert.match(reuseBlock, /if: steps\.download_reuse_artifact\.outcome == 'success'/);
  assert.match(reuseBlock, /verify_release_reuse\.mjs/);
  assert.match(reuseBlock, /--root release-candidate/);
  assert.match(reuseBlock, /--repository "\$RELEASE_REPOSITORY"/);
  assert.equal(
    buildJob.match(/if: steps\.verify_reuse_artifact\.outcome != 'success'/g)?.length,
    5,
    "Rust validation, build, and upload must all be conditional on verified artifact reuse",
  );
  const uploadIndex = buildJob.indexOf("Upload immutable release artifact");
  assert.ok(uploadIndex > signingIndex, "the artifact must upload after the conditional build");
  assert.match(buildJob.slice(uploadIndex), /overwrite: false/);

  assert.match(publishJob, /needs: build/);
  assert.match(publishJob, /environment: cadence-production/);
  assert.match(publishJob, /\n    permissions:\n      contents: write\n      actions: read\n/);
  assert.match(publishJob, /- name: Check out source for release publisher\n        uses: actions\/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4\.4\.0\n        with:\n          ref: \$\{\{ needs\.build\.outputs\.source_sha \}\}\n          persist-credentials: false/);
  assert.match(publishJob, /actions\/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4\.3\.0/);
  assert.match(publishJob, /name: \$\{\{ needs\.build\.outputs\.artifact_upload_name \}\}/);
  assert.match(publishJob, /github-token: \$\{\{ github\.token \}\}/);
  assert.match(publishJob, /run-id: \$\{\{ github\.run_id \}\}/);
  assert.match(publishJob, /\.source\.git_sha == \$git_sha/);
  assert.doesNotMatch(publishJob, /and \.git_sha == \$git_sha/);

  const publishJobEnvStart = publishJob.indexOf("    env:\n");
  const publishStepsStart = publishJob.indexOf("    steps:\n");
  assert.ok(
    publishJobEnvStart >= 0 && publishStepsStart > publishJobEnvStart,
    "publish job must have a distinct job-level env block",
  );
  const publishJobEnv = publishJob.slice(publishJobEnvStart, publishStepsStart);
  assert.doesNotMatch(
    publishJobEnv,
    /CADENCE_RELEASE_UPLOAD_TOKEN|CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN/,
    "PortalSurfer upload credentials must not be job-scoped",
  );

  const tagVerificationStep = workflowRunBlock(workflow, "Verify release tag target");
  const tagVerificationStepIndex = publishJob.indexOf("Verify release tag target");
  const githubReleaseIndex = publishJob.indexOf("Create or verify GitHub release idempotently");
  assert.ok(tagVerificationStepIndex >= 0 && tagVerificationStepIndex < githubReleaseIndex);
  assert.match(tagVerificationStep, /verify_tag_target\.sh/);
  assert.match(tagVerificationStep, /--repository "\$GITHUB_REPOSITORY"/);
  assert.match(tagVerificationStep, /--tag "\$RELEASE_TAG"/);
  assert.match(tagVerificationStep, /--source-sha "\$SOURCE_SHA"/);
  assert.match(publishJob, /gh release view/);
  assert.match(publishJob, /gh release create/);
  assert.doesNotMatch(publishJob, /git\/ref\/tags\/\$RELEASE_TAG/);
  assert.match(publishJob, /--verify-tag/);
  assert.match(publishJob, /gh release download/);
  assert.doesNotMatch(publishJob, /build_macos_release\.sh|cargo \+stable|cargo build/);

  const portalSurferIndex = publishJob.indexOf("Publish exact release to PortalSurfer");
  assert.ok(githubReleaseIndex >= 0 && githubReleaseIndex < portalSurferIndex, "PortalSurfer publication must be last");
  const publisherStepEnd = publishJob.indexOf("\n      - name: ", portalSurferIndex);
  assert.equal(publisherStepEnd, -1, "PortalSurfer publication must remain the final publish step");
  const publisherStep = publishJob.slice(portalSurferIndex);
  assert.match(
    publisherStep,
    /Publish exact release to PortalSurfer\n        env:\n          CADENCE_RELEASE_UPLOAD_TOKEN: \$\{\{ secrets\.CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN \}\}\n        run: \|\n/,
    "PortalSurfer upload credentials must be scoped to the final publisher step",
  );
  assert.equal(
    workflow.match(/\$\{\{ secrets\.CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN \}\}/g)?.length,
    1,
    "the PortalSurfer production secret must occur exactly once in the workflow",
  );
  const beforePublisher = publishJob.slice(0, portalSurferIndex);
  assert.doesNotMatch(
    beforePublisher,
    /Verify PortalSurfer upload token|CADENCE_RELEASE_UPLOAD_TOKEN|CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN/,
    "PortalSurfer token validation and references must not precede the final publisher",
  );
  assert.match(
    publisherStep,
    /Publish exact release to PortalSurfer\n        env:\n          CADENCE_RELEASE_UPLOAD_TOKEN: \$\{\{ secrets\.CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN \}\}\n        run: \|\n          node scripts\/release\/publish_release\.mjs \\\n            --manifest release-output\/cadence-release-manifest\.json \\\n            --root release-output/,
    "publisher arguments must remain separate shell arguments after YAML parsing",
  );

  const releaseJobs = `${buildJob}\n${publishJob}`;
  for (const legacySecret of [
    "APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64",
    "APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD",
    "APPLE_NOTARY_KEY_BASE64",
    "APPLE_NOTARY_KEY_ID",
    "APPLE_NOTARY_ISSUER_ID",
    "CADENCE_RELEASE_UPLOAD_TOKEN",
  ]) {
    assert.doesNotMatch(
      releaseJobs,
      new RegExp(`\\$\\{\\{ secrets\\.${legacySecret} \\}\\}`),
      `release jobs must not use the legacy repository secret ${legacySecret}`,
    );
  }
});

test("automatic nightly IDs use final source while reservation tags retain original source", async () => {
  const workflow = await fs.readFile(workflowPath, "utf8");
  const reservationRun = workflowRunBlock(workflow, "Reserve nightly source");
  const prepareRun = workflowRunBlock(workflow, "Select release metadata");
  const newReservationSourceAssignment = 'source_sha="$(git rev-parse HEAD)"';
  const preparedSourceAssignment = 'source_sha="$(printf \x27%s\x27 "$RESERVED_SOURCE_SHA"';
  const buildIdDerivation = 'short_sha="${source_sha:0:12}"';
  const sourceAssignmentIndex = reservationRun.indexOf(newReservationSourceAssignment);
  const pushIndex = reservationRun.indexOf('git push --atomic origin HEAD:refs/heads/main');
  const preparedSourceIndex = prepareRun.indexOf(preparedSourceAssignment);
  const buildIdIndex = prepareRun.indexOf(buildIdDerivation);
  assert.ok(sourceAssignmentIndex >= 0, "new nightly reservations must assign the final source SHA");
  assert.ok(pushIndex >= 0 && sourceAssignmentIndex > pushIndex, "the final source SHA must follow the reservation push");
  assert.ok(preparedSourceIndex >= 0, "metadata preparation must consume the reserved source SHA");
  assert.ok(buildIdIndex > preparedSourceIndex, "final source SHA must be assigned before build ID derivation");
  assert.match(reservationRun, /reservation_tag="cadence-nightly-\${RUN_NUMBER}-\${original_source_sha:0:12}"/);

  const buildIdBlockEnd = prepareRun.indexOf("\n\n", buildIdIndex);
  assert.ok(buildIdBlockEnd > buildIdIndex, "workflow must contain a bounded build ID derivation block");
  const buildIdBlock = prepareRun.slice(buildIdIndex, buildIdBlockEnd);
  const originalSourceSha = "a".repeat(40);
  const finalSourceSha = "b".repeat(40);
  const runNumber = "42";
  const buildScript = [
    "set -euo pipefail",
    `original_source_sha='${originalSourceSha}'`,
    `source_sha='${finalSourceSha}'`,
    "channel=nightly",
    `RUN_NUMBER='${runNumber}'`,
    "version=0.1.1-nightly.42",
    "tag_release=true",
    `reservation_tag="cadence-nightly-${runNumber}-${originalSourceSha.slice(0, 12)}"`,
    "release_tag=\"$reservation_tag\"",
    buildIdBlock,
    "printf '%s\\t%s\\n' \"$build_id\" \"$release_tag\"",
  ].join("\n");
  const { stdout } = await execFileAsync("bash", ["-c", buildScript]);
  assert.equal(
    stdout.trim(),
    `cadence-nightly-${runNumber}-${finalSourceSha.slice(0, 12)}\tcadence-nightly-${runNumber}-${originalSourceSha.slice(0, 12)}`,
  );
});

test("release tag verification is universal across stable, rc, and nightly tag paths", async () => {
  const workflow = await fs.readFile(workflowPath, "utf8");
  const prepareRun = workflowRunBlock(workflow, "Select release metadata");
  const reservationRun = workflowRunBlock(workflow, "Reserve nightly source");
  const publishJob = workflow.slice(workflow.indexOf("\n  publish:\n"));
  const verifierIndex = publishJob.indexOf("Verify release tag target");
  const releaseIndex = publishJob.indexOf("Create or verify GitHub release idempotently");

  assert.ok(verifierIndex >= 0 && verifierIndex < releaseIndex, "tag verification must precede release view/create");
  assert.match(publishJob.slice(verifierIndex, releaseIndex), /if: needs\.build\.outputs\.create_github_release == 'true'/);
  assert.match(prepareRun, /channel=stable/);
  assert.match(prepareRun, /channel=rc/);
  assert.match(prepareRun, /channel=nightly/);
  assert.match(prepareRun, /tag_release=true/);
  assert.match(prepareRun, /automatic_nightly=true/);
  assert.match(prepareRun, /release_tag="\$reservation_tag"/);
  assert.match(reservationRun, /reservation_tag="cadence-nightly-/);
  assert.match(prepareRun, /create_github_release=true/);
});
