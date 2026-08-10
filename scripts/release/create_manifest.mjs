#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const BASE_VERSION_RE = "(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)";
const CHANNEL_VERSION_RES = {
  stable: new RegExp(`^${BASE_VERSION_RE}$`),
  rc: new RegExp(`^${BASE_VERSION_RE}-rc\\.[1-9]\\d*$`),
  nightly: new RegExp(`^${BASE_VERSION_RE}-nightly\\.[1-9]\\d*$`),
};
const RELEASE_CHANNELS = new Set(Object.keys(CHANNEL_VERSION_RES));
const SHA_RE = /^[0-9a-f]{40}$/;
const TEAM_ID_RE = /^[A-Z0-9]{10}$/;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

function usage(message) {
  if (message) console.error(`error: ${message}`);
  console.error("usage: create_manifest.mjs --output-dir DIR --version VERSION --channel stable|rc|nightly --build-id ID --git-sha SHA --released-at ISO --team-id TEAM --notary-submission-id UUID");
  process.exit(2);
}

const args = process.argv.slice(2);
const values = {};
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (!argument.startsWith("--") || index + 1 >= args.length) usage(`unknown argument ${argument}`);
  values[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = args[++index];
}

const outputDir = path.resolve(values.outputDir || "");
const version = String(values.version || "");
const channel = String(values.channel || "stable");
const buildId = String(values.buildId || "");
const gitSha = String(values.gitSha || "").toLowerCase();
const releasedAt = String(values.releasedAt || "");
const teamId = String(values.teamId || "");
const notarySubmissionId = String(values.notarySubmissionId || "").toLowerCase();
if (!outputDir || !version) usage("version is required");
if (!RELEASE_CHANNELS.has(channel)) usage("channel must be stable, rc, or nightly");
if (!CHANNEL_VERSION_RES[channel].test(version)) usage(`${channel} version must use its matching semantic version syntax`);
if (!/^[a-z0-9][a-z0-9._-]{1,127}$/.test(buildId)) usage("build-id is not safe for the release API");
if (!SHA_RE.test(gitSha)) usage("git-sha must be a 40-character lowercase commit SHA");
if (Number.isNaN(new Date(releasedAt).getTime())) usage("released-at must be an ISO timestamp");
if (!TEAM_ID_RE.test(teamId)) usage("team-id must be ten uppercase letters or digits");
if (!UUID_RE.test(notarySubmissionId)) usage("notary-submission-id must be a UUID");

const artifactName = `cadence-v${version}-macos-arm64.zip`;
const screenshotName = "cadence-default-ui-1594x987.png";
const changelogName = "CHANGELOG.md";
const manifestName = "cadence-release-manifest.json";

async function descriptor(name, mediaType) {
  const bytes = await fs.readFile(path.join(outputDir, name));
  return {
    name,
    media_type: mediaType,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    size_bytes: bytes.length,
  };
}

async function pngDimensions(name) {
  const bytes = await fs.readFile(path.join(outputDir, name));
  if (bytes.length < 33 || bytes.subarray(0, 8).toString("hex") !== "89504e470d0a1a0a") throw new Error(`${name} is not a PNG`);
  if (bytes.subarray(12, 16).toString("ascii") !== "IHDR" || bytes.readUInt32BE(8) !== 13) throw new Error(`${name} has no valid IHDR`);
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (!width || !height || bytes[24] !== 8 || ![2, 6].includes(bytes[25])) throw new Error(`${name} must be an 8-bit RGB/RGBA PNG`);
  return { width, height };
}

const artifact = await descriptor(artifactName, "application/zip");
artifact.format = "app";
artifact.platform = "macos";
artifact.architectures = ["arm64"];
const screenshot = await descriptor(screenshotName, "image/png");
const dimensions = await pngDimensions(screenshotName);
const changelog = await descriptor(changelogName, "text/markdown; charset=utf-8");
changelog.format = "markdown";

const manifest = {
  schema_version: 2,
  product: "cadence",
  build_id: buildId,
  version,
  channel,
  released_at: new Date(releasedAt).toISOString(),
  source: { repository: "PORTALSURFER/cadence", git_sha: gitSha, dirty: false },
  distribution: "production",
  signing: {
    identity_class: "Developer ID Application",
    notarized: true,
    stapled: true,
    team_id: teamId,
    notary_submissions: { app: notarySubmissionId },
  },
  artifacts: [artifact],
  screenshot: {
    role: "default-ui",
    ...screenshot,
    width: dimensions.width,
    height: dimensions.height,
    logical_width: dimensions.width,
    logical_height: dimensions.height,
    dpi_scale: 1.0,
    source_git_sha: gitSha,
  },
  changelog,
};

await fs.writeFile(path.join(outputDir, manifestName), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
console.log(JSON.stringify({ manifest: path.join(outputDir, manifestName), build_id: buildId, artifact: artifactName }, null, 2));
