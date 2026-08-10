#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);
const releaseDirectory = path.dirname(fileURLToPath(import.meta.url));
const buildScript = path.join(releaseDirectory, "build_macos_release.sh");
const manifestScript = path.join(releaseDirectory, "create_manifest.mjs");
const workflowPath = path.join(releaseDirectory, "..", "..", ".github", "workflows", "release.yml");
const gitSha = "a".repeat(40);
const notarySubmissionId = "00000000-0000-4000-8000-000000000000";
const png = Buffer.from("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489", "hex");

async function createInputDirectory(version) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "cadence-manifest-test-"));
  await fs.writeFile(path.join(directory, `cadence-v${version}-macos-arm64.zip`), "zip fixture\n");
  await fs.writeFile(path.join(directory, "cadence-default-ui-1594x987.png"), png);
  await fs.writeFile(path.join(directory, "CHANGELOG.md"), "# Test release\n");
  return directory;
}

async function createManifest(directory, version, channel) {
  const args = [
    manifestScript,
    "--output-dir", directory,
    "--version", version,
    "--build-id", "cadence-test-build",
    "--git-sha", gitSha,
    "--released-at", "2026-08-09T00:00:00Z",
    "--team-id", "TEAM123456",
    "--notary-submission-id", notarySubmissionId,
  ];
  if (channel) args.push("--channel", channel);
  await execFileAsync(process.execPath, args);
  return JSON.parse(await fs.readFile(path.join(directory, "cadence-release-manifest.json"), "utf8"));
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

test("release workflow keeps signing and publication on separate immutable jobs", async () => {
  const workflow = await fs.readFile(workflowPath, "utf8");
  const buildMatch = workflow.match(/\n  build:\n([\s\S]*?)\n  publish:\n/);
  assert.ok(buildMatch, "workflow must define build and publish jobs");
  const buildJob = buildMatch[1];
  const publishJob = workflow.slice(buildMatch.index + buildMatch[0].length - "  publish:\n".length);

  for (const output of ["channel", "version", "build_id", "release_tag", "tag_release", "create_github_release", "artifact_upload_name", "source_sha"]) {
    assert.match(buildJob, new RegExp(`${output}: \\$\\{\\{ steps\\.release\\.outputs\\.${output} \\}\\}`));
  }
  assert.match(buildJob, /runs-on: macos-14/);
  assert.match(buildJob, /\n    permissions:\n      contents: read\n      actions: read\n/);
  assert.match(buildJob, /- name: Check out source\n        uses: actions\/checkout@v4\n        with:\n          persist-credentials: false/);
  assert.match(buildJob, /actions\/upload-artifact@v4/);
  assert.match(buildJob, /path: release-output\//);
  for (const secret of [
    "APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64",
    "APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD",
    "APPLE_NOTARY_KEY_BASE64",
    "APPLE_NOTARY_KEY_ID",
    "APPLE_NOTARY_ISSUER_ID",
  ]) {
    assert.match(buildJob, new RegExp(secret));
  }
  assert.doesNotMatch(buildJob, /CADENCE_RELEASE_UPLOAD_TOKEN|PORTALSURFER_RELEASE_ENDPOINT|publish_release\.mjs|gh release/);

  const reuseIndex = buildJob.indexOf("Reuse existing release artifact when available");
  const signingIndex = buildJob.indexOf("Build, sign, notarize, and describe release");
  assert.ok(reuseIndex >= 0 && reuseIndex < signingIndex, "reuse must be attempted before signing");
  const reuseBlock = buildJob.slice(reuseIndex, signingIndex);
  assert.match(reuseBlock, /id: reuse_artifact/);
  assert.match(reuseBlock, /continue-on-error: true/);
  assert.match(reuseBlock, /actions\/download-artifact@v4/);
  assert.match(reuseBlock, /name: \$\{\{ steps\.release\.outputs\.artifact_upload_name \}\}/);
  assert.match(reuseBlock, /path: release-output\n/);
  assert.match(reuseBlock, /github-token: \$\{\{ github\.token \}\}/);
  assert.match(reuseBlock, /run-id: \$\{\{ github\.run_id \}\}/);
  assert.equal(
    buildJob.match(/if: steps\.reuse_artifact\.outcome != 'success'/g)?.length,
    2,
    "build and upload must both be conditional on artifact reuse",
  );
  const uploadIndex = buildJob.indexOf("Upload immutable release artifact");
  assert.ok(uploadIndex > signingIndex, "the artifact must upload after the conditional build");
  assert.match(buildJob.slice(uploadIndex), /overwrite: false/);

  assert.match(publishJob, /needs: build/);
  assert.match(publishJob, /\n    permissions:\n      contents: write\n      actions: read\n/);
  assert.match(publishJob, /- name: Check out source for release publisher\n        uses: actions\/checkout@v4\n        with:\n          persist-credentials: false/);
  assert.match(publishJob, /actions\/download-artifact@v4/);
  assert.match(publishJob, /name: \$\{\{ needs\.build\.outputs\.artifact_upload_name \}\}/);
  assert.match(publishJob, /github-token: \$\{\{ github\.token \}\}/);
  assert.match(publishJob, /run-id: \$\{\{ github\.run_id \}\}/);
  assert.match(publishJob, /\.source\.git_sha == \$git_sha/);
  assert.doesNotMatch(publishJob, /and \.git_sha == \$git_sha/);
  assert.match(publishJob, /CADENCE_RELEASE_UPLOAD_TOKEN/);
  assert.match(publishJob, /gh release view/);
  assert.match(publishJob, /gh release create/);
  assert.match(publishJob, /gh release download/);
  assert.doesNotMatch(publishJob, /build_macos_release\.sh|cargo \+stable|cargo build/);

  const githubReleaseIndex = publishJob.indexOf("Create or verify GitHub release idempotently");
  const portalSurferIndex = publishJob.indexOf("Publish exact release to PortalSurfer");
  assert.ok(githubReleaseIndex >= 0 && githubReleaseIndex < portalSurferIndex, "PortalSurfer publication must be last");
});
