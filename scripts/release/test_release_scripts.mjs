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
const allocatorScript = path.join(releaseDirectory, "allocate_nightly_version.sh");
const buildScript = path.join(releaseDirectory, "build_macos_release.sh");
const manifestScript = path.join(releaseDirectory, "create_manifest.mjs");
const workflowPath = path.join(releaseDirectory, "..", "..", ".github", "workflows", "release.yml");
const gitSha = "a".repeat(40);
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
      execFileAsync(buildScript, ["--version", "0.1.0", "--output-dir", visibleNonemptyDirectory]),
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
    script.indexOf('validate_release_output_dir "$output_dir" "$caller_cwd"')
      < script.indexOf("require_env APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64"),
  );
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
  const prepareMatch = workflow.match(/\n  prepare:\n([\s\S]*?)\n  build:\n/);
  const buildMatch = workflow.match(/\n  build:\n([\s\S]*?)\n  publish:\n/);
  assert.ok(prepareMatch, "workflow must define a preparation job");
  assert.ok(buildMatch, "workflow must define build and publish jobs");
  const prepareJob = prepareMatch[1];
  const buildJob = buildMatch[1];
  const publishJob = workflow.slice(buildMatch.index + buildMatch[0].length - "  publish:\n".length);

  assert.match(prepareJob, /runs-on: ubuntu-latest/);
  assert.match(prepareJob, /\n    permissions:\n      contents: write\n/);
  assert.doesNotMatch(prepareJob, /actions: read/);
  assert.match(prepareJob, /ref: \$\{\{ github\.sha \}\}/);
  assert.match(prepareJob, /gh api --paginate/);
  assert.match(prepareJob, /select\(\.draft == false and \.published_at != null\)/);
  assert.match(prepareJob, /allocate_nightly_version\.sh/);
  assert.match(prepareJob, /original_lock_version/);
  assert.match(prepareJob, /Cargo\.toml and Cargo\.lock Cadence package versions differ/);
  assert.match(prepareJob, /cadence-nightly-\$\{RUN_NUMBER\}-\$\{original_source_sha:0:12\}/);
  assert.match(prepareJob, /refs\/heads\/main/);
  assert.match(prepareJob, /write_reserved_package_version/);
  assert.match(prepareJob, /git push --atomic origin HEAD:refs\/heads\/main "refs\/tags\/\$reservation_tag"/);
  assert.match(prepareJob, /git add Cargo\.toml Cargo\.lock/);
  assert.match(prepareJob, /git diff --cached --name-only/);

  for (const output of ["channel", "version", "build_id", "release_tag", "tag_release", "create_github_release", "artifact_upload_name", "source_sha"]) {
    assert.match(prepareJob, new RegExp(`${output}: \\$\\{\\{ steps\\.release\\.outputs\\.${output} \\}\\}`));
    assert.match(buildJob, new RegExp(`${output}: \\$\\{\\{ needs\\.prepare\\.outputs\\.${output} \\}\\}`));
  }
  assert.match(buildJob, /needs: prepare/);
  assert.match(buildJob, /runs-on: macos-14/);
  assert.match(buildJob, /environment: cadence-production/);
  assert.match(buildJob, /\n    permissions:\n      contents: read\n      actions: read\n/);
  assert.match(buildJob, /- name: Check out source\n        uses: actions\/checkout@v4\n        with:\n          ref: \$\{\{ needs\.prepare\.outputs\.source_sha \}\}\n          persist-credentials: false/);
  assert.doesNotMatch(buildJob, /Select release metadata|github\.event_name|steps\.release/);
  assert.match(buildJob, /GITHUB_SHA: \$\{\{ needs\.prepare\.outputs\.source_sha \}\}/);
  assert.match(buildJob, /export GITHUB_SHA=/);
  assert.match(buildJob, /actions\/upload-artifact@v4/);
  assert.match(buildJob, /path: release-output\//);
  for (const secret of [
    "CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64",
    "CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD",
    "CADENCE_PRODUCTION_APPLE_NOTARY_KEY_BASE64",
    "CADENCE_PRODUCTION_APPLE_NOTARY_KEY_ID",
    "CADENCE_PRODUCTION_APPLE_NOTARY_ISSUER_ID",
  ]) {
    assert.match(buildJob, new RegExp(`\\$\\{\\{ secrets\\.${secret} \\}\\}`));
  }
  assert.doesNotMatch(buildJob, /CADENCE_RELEASE_UPLOAD_TOKEN|PORTALSURFER_RELEASE_ENDPOINT|publish_release\.mjs|gh release/);

  const reuseIndex = buildJob.indexOf("Reuse existing release artifact when available");
  const signingIndex = buildJob.indexOf("Build, sign, notarize, and describe release");
  assert.ok(reuseIndex >= 0 && reuseIndex < signingIndex, "reuse must be attempted before signing");
  const reuseBlock = buildJob.slice(reuseIndex, signingIndex);
  assert.match(reuseBlock, /id: reuse_artifact/);
  assert.match(reuseBlock, /continue-on-error: true/);
  assert.match(reuseBlock, /actions\/download-artifact@v4/);
  assert.match(reuseBlock, /name: \$\{\{ needs\.prepare\.outputs\.artifact_upload_name \}\}/);
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
  assert.match(publishJob, /environment: cadence-production/);
  assert.match(publishJob, /\n    permissions:\n      contents: write\n      actions: read\n/);
  assert.match(publishJob, /- name: Check out source for release publisher\n        uses: actions\/checkout@v4\n        with:\n          ref: \$\{\{ needs\.build\.outputs\.source_sha \}\}\n          persist-credentials: false/);
  assert.match(publishJob, /actions\/download-artifact@v4/);
  assert.match(publishJob, /name: \$\{\{ needs\.build\.outputs\.artifact_upload_name \}\}/);
  assert.match(publishJob, /github-token: \$\{\{ github\.token \}\}/);
  assert.match(publishJob, /run-id: \$\{\{ github\.run_id \}\}/);
  assert.match(publishJob, /\.source\.git_sha == \$git_sha/);
  assert.doesNotMatch(publishJob, /and \.git_sha == \$git_sha/);
  assert.match(
    publishJob,
    /\$\{\{ secrets\.CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN \}\}/,
  );
  assert.match(publishJob, /gh release view/);
  assert.match(publishJob, /gh release create/);
  assert.match(publishJob, /git\/ref\/tags\/\$RELEASE_TAG/);
  assert.match(publishJob, /--verify-tag/);
  assert.match(publishJob, /gh release download/);
  assert.doesNotMatch(publishJob, /build_macos_release\.sh|cargo \+stable|cargo build/);

  const githubReleaseIndex = publishJob.indexOf("Create or verify GitHub release idempotently");
  const portalSurferIndex = publishJob.indexOf("Publish exact release to PortalSurfer");
  assert.ok(githubReleaseIndex >= 0 && githubReleaseIndex < portalSurferIndex, "PortalSurfer publication must be last");
  assert.match(
    publishJob,
    /- name: Publish exact release to PortalSurfer\n        run: \|\n          node scripts\/release\/publish_release\.mjs \\\n            --manifest release-output\/cadence-release-manifest\.json \\\n            --root release-output/,
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
