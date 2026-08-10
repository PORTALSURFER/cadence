#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const MANIFEST_CONTENT_TYPE = "application/vnd.portalsurfer.release-manifest+json;version=2";
const SHA_RE = /^[0-9a-f]{64}$/;

function usage(message) {
  if (message) console.error(`error: ${message}`);
  console.error("usage: publish_release.mjs --manifest PATH --root DIR [--endpoint URL] [--token TOKEN]");
  process.exit(2);
}

function endpointOrigin(value) {
  let parsed;
  try { parsed = new URL(value); } catch { throw new Error("endpoint must be https://portalsurfer.org or an explicit loopback URL"); }
  const loopback = parsed.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]", "::1"].includes(parsed.hostname);
  if ((parsed.protocol !== "https:" || parsed.hostname !== "portalsurfer.org") && !loopback) throw new Error("endpoint must be https://portalsurfer.org or an explicit loopback URL");
  if (parsed.pathname !== "/" || parsed.search || parsed.hash || parsed.username || parsed.password) throw new Error("endpoint must be an origin without a path");
  return parsed.origin;
}

const args = process.argv.slice(2);
const values = {};
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (!argument.startsWith("--") || index + 1 >= args.length) usage(`unknown argument ${argument}`);
  values[argument.slice(2).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = args[++index];
}
if (!values.manifest) usage("manifest is required");
const endpoint = endpointOrigin(String(values.endpoint || process.env.PORTALSURFER_RELEASE_ENDPOINT || "https://portalsurfer.org"));
const token = String(values.token || process.env.CADENCE_RELEASE_UPLOAD_TOKEN || "");
if (!token) usage("an upload token is required");
const manifestPath = path.resolve(values.manifest);
const root = path.resolve(values.root || path.dirname(manifestPath));
const manifestBytes = await fs.readFile(manifestPath);
const manifest = JSON.parse(manifestBytes.toString("utf8"));
if (!manifest || manifest.schema_version !== 2 || manifest.product !== "cadence" || !manifest.build_id) usage("manifest must be Cadence schema 2");
const descriptors = [...(manifest.artifacts || []), manifest.screenshot, manifest.changelog];
if (descriptors.length !== 3 || manifest.artifacts?.length !== 1 || manifest.artifacts[0]?.format !== "app" || manifest.artifacts[0]?.platform !== "macos" || manifest.artifacts[0]?.architectures?.join(",") !== "arm64" || new Set(descriptors.map((descriptor) => descriptor?.name)).size !== 3) usage("manifest must contain one macOS arm64 app artifact, one screenshot, and one changelog");

const capabilityUrl = `${endpoint}/plugins/api/v1/products/cadence/releases`;
const capabilityResponse = await fetch(capabilityUrl, { headers: { Accept: "application/json" }, cache: "no-store" });
if (!capabilityResponse.ok) throw new Error(`Cadence release capability check failed (${capabilityResponse.status}): ${await capabilityResponse.text()}`);
const capability = await capabilityResponse.json();
if (capability.artifact_kind !== "application" || !capability.release_upload?.manifest_schema_versions?.includes(2)) throw new Error("PortalSurfer does not advertise the Cadence application release contract");

const metadata = {
  Authorization: `Bearer ${token}`,
  "X-PortalSurfer-Release-Version": String(manifest.version || ""),
  "X-PortalSurfer-Release-Channel": String(manifest.channel || ""),
  "X-PortalSurfer-Released-At": String(manifest.released_at || ""),
};
for (const descriptor of descriptors) {
  if (!descriptor?.name || path.basename(descriptor.name) !== descriptor.name || descriptor.name.startsWith(".") || !SHA_RE.test(descriptor.sha256) || !Number.isInteger(descriptor.size_bytes) || descriptor.size_bytes <= 0) usage(`invalid descriptor for ${descriptor?.name || "unnamed file"}`);
  const filePath = path.join(root, descriptor.name);
  const bytes = await fs.readFile(filePath);
  const digest = crypto.createHash("sha256").update(bytes).digest("hex");
  if (digest !== descriptor.sha256 || bytes.length !== descriptor.size_bytes) throw new Error(`manifest metadata does not match ${descriptor.name}`);
  const url = `${endpoint}/plugins/api/v1/products/cadence/release-uploads/${encodeURIComponent(manifest.build_id)}/staging/files/${encodeURIComponent(descriptor.name)}`;
  const response = await fetch(url, {
    method: "PUT",
    headers: { ...metadata, "Content-Type": "application/octet-stream", "Content-Length": String(bytes.length), "X-PortalSurfer-Sha256": digest },
    body: bytes,
  });
  if (!response.ok) throw new Error(`staging ${descriptor.name} failed (${response.status}): ${await response.text()}`);
}

const manifestSha256 = crypto.createHash("sha256").update(manifestBytes).digest("hex");
const commitUrl = `${endpoint}/plugins/api/v1/products/cadence/release-uploads/${encodeURIComponent(manifest.build_id)}/commit`;
const commit = await fetch(commitUrl, {
  method: "PUT",
  headers: { ...metadata, "Content-Type": MANIFEST_CONTENT_TYPE, "Content-Length": String(manifestBytes.length), "X-PortalSurfer-Manifest-Sha256": manifestSha256 },
  body: manifestBytes,
});
if (!commit.ok) throw new Error(`Cadence release commit failed (${commit.status}): ${await commit.text()}`);
console.log(JSON.stringify(await commit.json(), null, 2));
