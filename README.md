# Cadence

Cadence is a local-first track review desk: import audio, listen, and capture a note at the current time. The GPUI native app is the root project.

## Run locally

On macOS, double-click [`Cadence.command`](./Cadence.command) to launch the native app through the existing Cargo/native runner. The canonical native development path is `cargo run` (or `cargo run --release`); Cargo stages the normal Cadence executable in a LaunchServices-visible `target/dev-app/Cadence.app` with the Cadence icon and bundle identifier, then launches that app. It stores native library metadata under the normal Cadence application-support directory.

To run it from Terminal:

```sh
./Cadence.command
```

To explicitly build the native macOS release bundle with the Cadence icon:

```sh
./scripts/build_native_app_bundle.sh
open dist/Cadence.app
```

The raw `target/debug/cadence-native` and `target/release/cadence-native` binaries are diagnostic artifacts only; they are not LaunchServices-visible app bundles.

## Production releases

Pushing a `vX.Y.Z` tag runs [the release workflow](.github/workflows/release.yml) on an Apple Silicon macOS runner. It runs the Rust test and Clippy gates, creates a versioned `Cadence.app`, signs it with Developer ID Application, submits it to Apple for notarization, staples and validates the ticket, creates `cadence-vX.Y.Z-macos-arm64.zip`, and publishes the schema-2 application manifest with channel `stable` to `https://portalsurfer.org`. Stable releases keep the existing tag-driven behavior and require the Cargo package version to match the tag exactly.

The same workflow runs nightly at `02:17 UTC` and can be started manually. Manual dispatch offers `stable` and `nightly`: stable requires an explicit `X.Y.Z` version (and only creates a GitHub release when dispatched from the matching `vX.Y.Z` tag), while nightly leaves the version blank and must run from `main`. Before an automatic nightly builds, its protected reservation job reads only published, non-draft GitHub releases across stable, RC, and nightly channels, reserves the next numeric patch, updates only the Cadence package entries in `Cargo.toml` and `Cargo.lock`, and atomically pushes that commit with the immutable `cadence-nightly-<run-number>-<12-character-original-commit>` tag. The nightly version is the reserved base plus `-nightly.<run-number>` (for example, `0.1.1-nightly.6`), and reruns reuse the same reservation, source commit, build id, tag, and artifact. The reservation tag is verified when the GitHub prerelease is published. A tag-triggered nightly may instead use an exact tag such as `v0.1.0-nightly.1`; explicit stable, RC, and tag-triggered nightly releases do not reserve or automatically bump the package version.

Release versions are channel-specific: stable is `X.Y.Z`, RC is `X.Y.Z-rc.N`, and nightly is `X.Y.Z-nightly.N`. Automatic nightlies use one globally increasing numeric patch stream across all three channels; existing nightlies are never renumbered. The full semantic version remains in the artifact name and manifest. Apple bundle metadata is separate: `CFBundleShortVersionString` is the numeric base `X.Y.Z`, while `CFBundleVersion` is the numeric build value (`X.Y.Z` for stable and `N` for RC/nightly). A failed or canceled run before the reservation consumes no patch; once the reservation commit and tag are pushed, later retries reuse that patch.

The nightly reservation, build, and publish jobs require the fixed `cadence-production` GitHub Actions environment. Configure that environment externally before a production release: allow only protected `main` and matching release tags, and require the reviewers selected in the environment protection rules. The metadata preparation job is read-only; the existing manual stable-source guard remains defense in depth and still accepts only `main` or the matching `vX.Y.Z` tag. Nightly and push-tag triggers retain their existing behavior.

The `cadence-production` environment needs these Actions secrets:

- `CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64`: a base64-encoded Developer ID Application `.p12` export.
- `CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD`: the `.p12` password.
- `CADENCE_PRODUCTION_APPLE_NOTARY_KEY_BASE64`, `CADENCE_PRODUCTION_APPLE_NOTARY_KEY_ID`, and `CADENCE_PRODUCTION_APPLE_NOTARY_ISSUER_ID`: an App Store Connect API key and its identifiers.
- `CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN`: the product-specific PortalSurfer release token.

For an existing setup, copy each value from the old repository-level secret to its corresponding `cadence-production` environment secret: `APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64` to `CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_BASE64`, `APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD` to `CADENCE_PRODUCTION_APPLE_DEVELOPER_ID_APPLICATION_CERT_PASSWORD`, `APPLE_NOTARY_KEY_BASE64` to `CADENCE_PRODUCTION_APPLE_NOTARY_KEY_BASE64`, `APPLE_NOTARY_KEY_ID` to `CADENCE_PRODUCTION_APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID` to `CADENCE_PRODUCTION_APPLE_NOTARY_ISSUER_ID`, and `CADENCE_RELEASE_UPLOAD_TOKEN` to `CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN`. After migration is verified, remove all of those old repository-level secrets. Do not expose secret values in repository files, logs, or documentation.

The release build derives the ten-character Team ID from the trailing `(TEAMID)` in the selected imported Developer ID Application identity; configure no separate Team ID secret. It fails closed if the identity does not contain a valid suffix.

GitHub Actions uses its built-in `GITHUB_TOKEN` for the GitHub release, so no separate GitHub release PAT is needed inside Actions. `CADENCE_PRODUCTION_PORTALSURFER_RELEASE_TOKEN` is still required for publishing the manifest and artifacts to PortalSurfer.

Optionally set the repository variable `PORTALSURFER_RELEASE_ENDPOINT`; it defaults to `https://portalsurfer.org`. The publisher accepts only that production origin or an explicit HTTP loopback URL for local testing. It checks the server capability before staging the app zip, screenshot, and `CHANGELOG.md`, then commits their hashes in the manifest.

The release builder accepts `--output-dir DIR` for a caller-selected artifact directory. Relative paths resolve from the caller's current working directory, and the parent directory must already exist. The target may be absent or an existing empty directory; symlinks, files, special nodes, nonempty directories (including hidden entries), `.`, `..`, `/`, and paths resolving to the repository root are rejected. The builder validates this before reading signing credentials and again immediately before output creation, and never recursively removes a caller-controlled output directory.

The local scripts are intentionally production-gated. Direct callers must pass a stable version exactly matching the root `cadence-native` package version; RC and nightly versions must use that same numeric `X.Y.Z` base, and `Cargo.lock` must agree with the package metadata. Check their syntax without contacting Apple or PortalSurfer:

```sh
bash -n scripts/build_native_app_bundle.sh scripts/release/allocate_nightly_version.sh scripts/release/build_macos_release.sh scripts/release/verify_tag_target.sh scripts/release/verify_macos_architecture.sh scripts/release/test_macos_architecture.sh
node --check scripts/release/create_manifest.mjs
node --check scripts/release/publish_release.mjs
node --check scripts/release/test_release_scripts.mjs
node --test scripts/release/test_release_scripts.mjs
bash scripts/release/test_bundle_version.sh
bash scripts/release/test_macos_architecture.sh
./scripts/release/build_macos_release.sh --help
```

A real release requires macOS, the protected `cadence-production` environment with the five Apple signing/notary secrets and the PortalSurfer upload token, and a clean checkout; the release tests exercise Team ID derivation and output-directory safety with synthetic identities and temporary directories without Apple credentials. No ad-hoc signature is accepted by the production release script. Direct callers may pass `--channel stable`, `--channel rc`, or `--channel nightly`; omitting it preserves the stable default.

To capture the current native window for visual refinement, run the macOS screenshot harness:

```sh
./scripts/capture_native_screenshot.sh artifacts/screenshots/cadence-native.png
```

It reuses an already-open Cadence native window or builds and launches the debug binary, then captures the titled window with macOS `screencapture`. Pass `--hover X Y` with screen coordinates before the output path to move the pointer and capture a hover state, for example `./scripts/capture_native_screenshot.sh --hover 150 210 --output artifacts/screenshots/import-hover.png`. Generated PNGs stay local under `artifacts/screenshots/`; the deterministic paint-plan tests remain the CI-safe visual contract.

The native app owns its library model and JSON persistence in the Cadence host, uses a native file picker and GPUI file-drop handling, decodes imported files off the UI thread, renders a waveform plus timestamped comments, and provides host-controlled playback. The former Radiant binary remains available for comparison with `cargo run --features radiant-legacy --bin cadence-radiant-legacy`.

## Native workspace

The recovered Technical Futurist Frame design is implemented in GPUI: compact charcoal cards, orange selections, aligned pill-bar waveforms, and Review, Planner and Tags views. The editable browser reference is in `design/frame`; its sample data is a design preview.

- Import audio through the Library plus button and Track Manager, or drop files onto the workspace. Original audio stays at its external path.
- Review main and reference waveforms, integrated LUFS, loudness matching, playback source and volume. The Spectrogram view uses a purple-to-yellow intensity heatmap, low frequencies at the bottom and newest audio at the right; on macOS, Metal computes its colors into a shared GPU surface that GPUI draws directly, with a cached CPU image fallback. Completed frames are reused while paused. Shift-drag a waveform to set a loop; Left/Right seeks one second when the waveform has focus.
- Add timestamped comments with `N`, or the waveform comment rail. A small editor opens beneath the marker while the rest of Review stays interactive. Drag the marker while typing; Save commits both its text and position, and Escape discards the draft. Edit, resolve and remove comments; switch between single-source and stacked-source comments.
- Move tracks through Backlog, Production, Mixdown and Mastering in Planner. Existing Groove, Arrangement and Polish values migrate to Production; Master and Release migrate to Mastering.
- Manage persistent colored task tags and album groups in Tags, with search and matching-track previews.
- Search, sort, filter and select tracks in Track Manager, then apply staged bulk changes to stage, tags and album groups. Removing a library entry retains the source audio file.
- Add and search comparison audio in Reference Manager, even before importing a main track. Rename catalog entries, replace or re-import their files, and see assignment counts. Replacements update assigned tracks; confirmed removal clears those assignments and retains the audio file. Files dropped in this tab join the reference catalog.
- Use Tab/Shift-Tab to navigate controls and modal fields. Focused volume controls support arrows and Home/End; Escape closes overlays. Space controls playback outside text fields.

A review run can use `CADENCE_DATA_DIR=/absolute/path/to/disposable-library` to isolate its library from the normal application-support directory. See `docs/frame-redesign-acceptance.md` for source provenance and current verification.

## Known limits

### Native app

The native app persists library metadata and the original external audio-file paths as JSON under the Cadence application-support directory, then analyzes a new source in a background worker to build a bounded retained waveform summary. Waveform and loudness analysis are cached on disk across restarts; the six most recently used analyses are also retained in memory. Unchanged sources reuse analysis after a quick source-stamp check, and changed sources are verified before reuse. Imports populate the cache so opening the newly imported track does not analyze it again. Moving or deleting a source file therefore requires re-importing it. The native launcher takes a single-process lock around that local library.

Native playback is responsive, host-controlled playback through Rodio. Each load is bound to the verified source ticket produced by waveform analysis and decodes from the handle opened for that ticket. An atomic replacement after a playback handle is opened may therefore finish the already-open original inode; any later end/seek reload reopens through the ticket, rejects the replacement, and stops with a source-changed error. Cadence does not copy, lock, or watch source files. The playhead displays the latest Rodio-reported position at GPUI update cadence; it is not a sample-accurate transport clock or a lock-free realtime audio engine. Rodio/CPAL may pull decoder data and service internal control state from the output callback, so occasional device-, decoder-, or system-load-related glitches remain possible. A future DSP, recording, monitoring, plugin-hosting, automation, low-latency scrubbing, or sample-accurate transport requirement would need a dedicated callback-safe backend. Native loudness uses bounded K-weighted integrated LUFS analysis decoded in the background; playback gain is applied after that analysis, so playback volume and reference matching do not change the displayed value. Reference tracks are stored as external paths and must be re-imported if moved or deleted; matching changes only reference playback gain and never rewrites either audio file. The Planner does not yet persist custom boards; stage is the only workflow state and favorites remain separate. The GPUI comment editor supports multiline text.
