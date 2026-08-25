#!/usr/bin/env node

import crypto from "node:crypto";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const releaseDirectory = path.dirname(fileURLToPath(import.meta.url));
const architectureVerifierScript = path.join(releaseDirectory, "verify_macos_architecture.sh");

const SHA256_RE = /^[0-9a-f]{64}$/;
const SHA_RE = /^[0-9a-f]{40}$/;
const TEAM_ID_RE = /^[A-Z0-9]{10}$/;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const BASE_VERSION_RE = "(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)\\.(?:0|[1-9][0-9]*)";
const CHANNEL_VERSION_RES = {
  stable: new RegExp(`^${BASE_VERSION_RE}$`),
  rc: new RegExp(`^${BASE_VERSION_RE}-rc\\.[1-9][0-9]*$`),
  nightly: new RegExp(`^${BASE_VERSION_RE}-nightly\\.[1-9][0-9]*$`),
};

const MANIFEST_NAME = "cadence-release-manifest.json";
const SCREENSHOT_NAME = "cadence-default-ui-1594x987.png";
const CHANGELOG_NAME = "CHANGELOG.md";
const SUMS_NAME = "SHA256SUMS.txt";
const BUNDLE_IDENTIFIER = "org.portalsurfer.cadence";

function usage(message) {
  if (message) console.error(`error: ${message}`);
  console.error("usage: verify_release_reuse.mjs --root DIR --version VERSION --channel stable|rc|nightly --build-id ID --source-sha SHA --repository OWNER/REPOSITORY");
  process.exit(2);
}

function parseArguments() {
  const values = {};
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (!argument.startsWith("--") || index + 1 >= args.length) usage(`unknown argument ${argument}`);
    values[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = args[++index];
  }
  for (const name of ["root", "version", "channel", "buildId", "sourceSha", "repository"]) {
    if (!values[name]) usage(`${name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)} is required`);
  }
  return values;
}

function fail(message) {
  throw new Error(`release reuse verification failed: ${message}`);
}

function expect(condition, message) {
  if (!condition) fail(message);
}

function expectObject(value, label) {
  expect(value !== null && typeof value === "object" && !Array.isArray(value), `${label} must be an object`);
}

function expectRegularFileStats(stats, name) {
  expect(stats.isFile() && !stats.isSymbolicLink(), `${name} must be a regular file`);
}

async function expectDirectory(directory, name) {
  let stats;
  try {
    stats = await fs.lstat(directory);
  } catch (error) {
    if (error?.code === "ENOENT") fail(`${name} is missing`);
    throw error;
  }
  expect(stats.isDirectory() && !stats.isSymbolicLink(), `${name} must be a real directory`);
}

async function expectRegularFile(filePath, name) {
  let stats;
  try {
    stats = await fs.lstat(filePath);
  } catch (error) {
    if (error?.code === "ENOENT") fail(`${name} is missing`);
    throw error;
  }
  expectRegularFileStats(stats, name);
  return stats;
}

async function expectExecutable(filePath, name) {
  const stats = await expectRegularFile(filePath, name);
  expect((stats.mode & 0o111) !== 0, `${name} must be executable`);
}

function commandErrorDetail(error) {
  const detail = [error?.stderr, error?.stdout, error?.message]
    .find((value) => typeof value === "string" && value.trim().length > 0);
  return detail ? `: ${detail.trim()}` : "";
}

async function runCommand(command, args, description) {
  try {
    return await execFileAsync(command, args, { maxBuffer: 1024 * 1024 });
  } catch (error) {
    fail(`${description}${commandErrorDetail(error)}`);
  }
}

async function readRegularFile(root, name) {
  const filePath = path.join(root, name);
  let stats;
  try {
    stats = await fs.lstat(filePath);
  } catch (error) {
    if (error?.code === "ENOENT") fail(`required file is missing: ${name}`);
    throw error;
  }
  expectRegularFileStats(stats, name);
  return { path: filePath, bytes: await fs.readFile(filePath), stats };
}

function descriptorShape(descriptor, name, mediaType) {
  expectObject(descriptor, `${name} descriptor`);
  expect(descriptor.name === name, `${name} descriptor has an unexpected name`);
  expect(descriptor.media_type === mediaType, `${name} descriptor has an unexpected media type`);
  expect(typeof descriptor.sha256 === "string" && SHA256_RE.test(descriptor.sha256), `${name} descriptor has an invalid SHA-256`);
  expect(Number.isSafeInteger(descriptor.size_bytes) && descriptor.size_bytes > 0, `${name} descriptor has an invalid size`);
  return descriptor;
}

function pngDimensions(bytes, name) {
  expect(bytes.length >= 33, `${name} is too short to be a PNG`);
  expect(bytes.subarray(0, 8).toString("hex") === "89504e470d0a1a0a", `${name} is not a PNG`);
  expect(bytes.readUInt32BE(8) === 13 && bytes.subarray(12, 16).toString("ascii") === "IHDR", `${name} has no valid IHDR`);
  expect(bytes[24] === 8 && [2, 6].includes(bytes[25]), `${name} must be an 8-bit RGB/RGBA PNG`);
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  expect(width > 0 && height > 0, `${name} must have positive dimensions`);
  return { width, height };
}

function verifyProductionProvenance(manifest, expected) {
  expectObject(manifest, "release manifest");
  expect(manifest.schema_version === 2, "manifest schema_version must be 2");
  expect(manifest.product === "cadence", "manifest product must be cadence");
  expect(manifest.version === expected.version, "manifest version does not match the prepared release");
  expect(manifest.channel === expected.channel, "manifest channel does not match the prepared release");
  expect(manifest.build_id === expected.buildId, "manifest build_id does not match the prepared release");
  expect(typeof manifest.released_at === "string" && !Number.isNaN(Date.parse(manifest.released_at)), "manifest released_at is invalid");
  expect(manifest.distribution === "production", "manifest distribution must be production");

  expectObject(manifest.source, "manifest source");
  expect(manifest.source.repository === expected.repository, "manifest source repository does not match the current repository");
  expect(manifest.source.git_sha === expected.sourceSha, "manifest source SHA does not match the prepared source");
  expect(manifest.source.dirty === false, "manifest source must be clean");

  expectObject(manifest.signing, "manifest signing");
  expect(manifest.signing.identity_class === "Developer ID Application", "manifest signing identity is not production Developer ID Application");
  expect(manifest.signing.notarized === true, "manifest must record successful notarization");
  expect(manifest.signing.stapled === true, "manifest must record a stapled notarization ticket");
  expect(typeof manifest.signing.team_id === "string" && TEAM_ID_RE.test(manifest.signing.team_id), "manifest signing team ID is invalid");
  expectObject(manifest.signing.notary_submissions, "manifest notary submissions");
  expect(typeof manifest.signing.notary_submissions.app === "string" && UUID_RE.test(manifest.signing.notary_submissions.app.toLowerCase()), "manifest app notarization submission ID is invalid");
}

function verifyDescriptors(manifest, expected) {
  expect(Array.isArray(manifest.artifacts) && manifest.artifacts.length === 1, "manifest must contain exactly one artifact descriptor");
  const artifactName = `cadence-v${expected.version}-macos-arm64.zip`;
  const artifact = descriptorShape(manifest.artifacts[0], artifactName, "application/zip");
  expect(artifact.format === "app", "artifact descriptor format must be app");
  expect(artifact.platform === "macos", "artifact descriptor platform must be macos");
  expect(Array.isArray(artifact.architectures) && artifact.architectures.length === 1 && artifact.architectures[0] === "arm64", "artifact descriptor architecture must be arm64");

  const screenshot = descriptorShape(manifest.screenshot, SCREENSHOT_NAME, "image/png");
  expect(manifest.screenshot.role === "default-ui", "screenshot descriptor role must be default-ui");
  expect(Number.isSafeInteger(screenshot.width) && screenshot.width > 0, "screenshot descriptor width is invalid");
  expect(Number.isSafeInteger(screenshot.height) && screenshot.height > 0, "screenshot descriptor height is invalid");
  expect(screenshot.logical_width === screenshot.width && screenshot.logical_height === screenshot.height, "screenshot logical dimensions must match its dimensions");
  expect(screenshot.dpi_scale === 1, "screenshot descriptor dpi_scale must be 1");
  expect(typeof screenshot.source_git_sha === "string" && SHA_RE.test(screenshot.source_git_sha), "screenshot source SHA is invalid");

  const changelog = descriptorShape(manifest.changelog, CHANGELOG_NAME, "text/markdown; charset=utf-8");
  expect(changelog.format === "markdown", "changelog descriptor format must be markdown");

  const descriptors = [artifact, screenshot, changelog];
  expect(new Set(descriptors.map((descriptor) => descriptor.name)).size === descriptors.length, "manifest descriptor names must be unique");
  return { artifact, screenshot, changelog, artifactName };
}

function verifySha256Sums(bytes, descriptors) {
  const lines = bytes.toString("utf8").replace(/\n$/, "").split("\n");
  expect(lines.length === descriptors.length, `${SUMS_NAME} must contain exactly one entry per release descriptor`);
  const expected = new Map(descriptors.map((descriptor) => [descriptor.name, descriptor.sha256]));
  const seen = new Set();
  for (const line of lines) {
    const match = line.match(/^([0-9a-f]{64})  (.+)$/);
    expect(match !== null, `${SUMS_NAME} contains a malformed entry`);
    const [, digest, name] = match;
    expect(expected.has(name) && !seen.has(name), `${SUMS_NAME} contains an unexpected or duplicate file`);
    expect(expected.get(name) === digest, `${SUMS_NAME} hash does not match the manifest for ${name}`);
    seen.add(name);
  }
  expect(seen.size === expected.size, `${SUMS_NAME} is missing a release descriptor`);
}

function expectedBundleMetadata(expected) {
  const shortVersion = expected.version.split("-", 1)[0];
  const buildNumber = expected.channel === "stable"
    ? shortVersion
    : expected.version.slice(expected.version.lastIndexOf(".") + 1);
  return {
    CFBundleIdentifier: BUNDLE_IDENTIFIER,
    CFBundleName: "Cadence",
    CFBundleExecutable: "Cadence",
    CFBundlePackageType: "APPL",
    CFBundleShortVersionString: shortVersion,
    CFBundleVersion: buildNumber,
  };
}

async function verifyBundleMetadata(infoPlistPath, expected) {
  const { stdout } = await runCommand(
    "/usr/bin/plutil",
    ["-convert", "json", "-o", "-", "--", infoPlistPath],
    "Cadence.app Info.plist is invalid",
  );
  let metadata;
  try {
    metadata = JSON.parse(stdout);
  } catch (error) {
    fail(`Cadence.app Info.plist did not convert to valid JSON: ${error.message}`);
  }
  expectObject(metadata, "Cadence.app Info.plist");
  for (const [key, value] of Object.entries(expectedBundleMetadata(expected))) {
    expect(metadata[key] === value, `Cadence.app Info.plist ${key} must be ${value}`);
  }
}

async function verifyReleaseBundle(artifactPath, expected) {
  expect(process.platform === "darwin", "release reuse verification requires macOS");
  const extractionDirectory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-release-reuse-"));
  try {
    await runCommand(
      "/usr/bin/ditto",
      ["-x", "-k", artifactPath, extractionDirectory],
      "artifact archive is not a valid ZIP",
    );

    const appPath = path.join(extractionDirectory, "Cadence.app");
    const contentsPath = path.join(appPath, "Contents");
    const macosPath = path.join(contentsPath, "MacOS");
    const executablePath = path.join(macosPath, "Cadence");
    const infoPlistPath = path.join(contentsPath, "Info.plist");
    await expectDirectory(appPath, "Cadence.app bundle");
    await expectDirectory(contentsPath, "Cadence.app Contents directory");
    await expectDirectory(macosPath, "Cadence.app MacOS directory");
    await expectExecutable(executablePath, "Cadence.app executable");
    await expectRegularFile(infoPlistPath, "Cadence.app Info.plist");
    await verifyBundleMetadata(infoPlistPath, expected);

    await runCommand(
      "/bin/bash",
      [architectureVerifierScript, executablePath],
      "Cadence.app executable architecture verification failed",
    );
    await runCommand(
      "codesign",
      ["--verify", "--strict", "--verbose=2", executablePath],
      "Cadence.app executable codesign verification failed",
    );
    await runCommand(
      "codesign",
      ["--verify", "--deep", "--strict", "--verbose=2", appPath],
      "Cadence.app bundle codesign verification failed",
    );
  } finally {
    await fs.rm(extractionDirectory, { recursive: true, force: true });
  }
}

async function verifyCandidate(values) {
  const expected = {
    version: String(values.version),
    channel: String(values.channel),
    buildId: String(values.buildId),
    sourceSha: String(values.sourceSha).toLowerCase(),
    repository: String(values.repository),
  };
  expect(CHANNEL_VERSION_RES[expected.channel]?.test(expected.version) === true, "prepared release version/channel is invalid");
  expect(SHA_RE.test(expected.sourceSha), "prepared source SHA is invalid");
  expect(expected.repository.match(/^[^/\s]+\/[^/\s]+$/) !== null, "prepared repository is invalid");
  expect(/^[a-z0-9][a-z0-9._-]{1,127}$/.test(expected.buildId), "prepared build ID is invalid");

  const root = path.resolve(String(values.root));
  const rootStats = await fs.lstat(root).catch((error) => {
    if (error?.code === "ENOENT") fail("candidate directory is missing");
    throw error;
  });
  expect(rootStats.isDirectory() && !rootStats.isSymbolicLink(), "candidate root must be a real directory");

  const requiredNames = new Set([
    MANIFEST_NAME,
    SCREENSHOT_NAME,
    CHANGELOG_NAME,
    SUMS_NAME,
    `cadence-v${expected.version}-macos-arm64.zip`,
  ]);
  const entries = await fs.readdir(root, { withFileTypes: true });
  for (const requiredName of requiredNames) {
    expect(entries.some((entry) => entry.name === requiredName), `required file is missing: ${requiredName}`);
  }
  expect(entries.length === requiredNames.size, "candidate directory contains unexpected files");
  for (const entry of entries) {
    expect(requiredNames.has(entry.name), `candidate directory contains unexpected file: ${entry.name}`);
    expect(entry.isFile() && !entry.isSymbolicLink(), `${entry.name} must be a regular file`);
  }

  const manifestFile = await readRegularFile(root, MANIFEST_NAME);
  let manifest;
  try {
    manifest = JSON.parse(manifestFile.bytes.toString("utf8"));
  } catch (error) {
    fail(`manifest is not valid JSON: ${error.message}`);
  }
  verifyProductionProvenance(manifest, expected);
  const descriptors = verifyDescriptors(manifest, expected);
  const screenshotFile = await readRegularFile(root, SCREENSHOT_NAME);
  const screenshotDimensions = pngDimensions(screenshotFile.bytes, SCREENSHOT_NAME);
  expect(screenshotDimensions.width === descriptors.screenshot.width && screenshotDimensions.height === descriptors.screenshot.height, "screenshot dimensions do not match the manifest");
  const sumsFile = await readRegularFile(root, SUMS_NAME);
  verifySha256Sums(sumsFile.bytes, [descriptors.artifact, descriptors.screenshot, descriptors.changelog]);

  for (const descriptor of [descriptors.artifact, descriptors.screenshot, descriptors.changelog]) {
    const file = await readRegularFile(root, descriptor.name);
    expect(file.bytes.length === descriptor.size_bytes, `${descriptor.name} size does not match the manifest`);
    const digest = crypto.createHash("sha256").update(file.bytes).digest("hex");
    expect(digest === descriptor.sha256, `${descriptor.name} SHA-256 does not match the manifest`);
    if (descriptor === descriptors.artifact) await verifyReleaseBundle(file.path, expected);
  }

  console.log(JSON.stringify({ verified: true, build_id: expected.buildId, root }, null, 2));
}

try {
  await verifyCandidate(parseArguments());
} catch (error) {
  console.error(error?.message || error);
  process.exitCode = 1;
}
