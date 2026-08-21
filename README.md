# Cadence

Cadence is a local-first track review desk: import audio, listen, and capture a note at the current time. The Rust/Radiant native app is the root project.

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

The same workflow runs nightly at `02:17 UTC` and can be started manually. Manual dispatch offers `stable` and `nightly`: stable requires an explicit `X.Y.Z` version (and only creates a GitHub release when dispatched from the matching `vX.Y.Z` tag), while nightly leaves the version blank and must run from `main`. Before an automatic nightly builds, its preparation job reads only published, non-draft GitHub releases across stable, RC, and nightly channels, reserves the next numeric patch, updates only the Cadence package entries in `Cargo.toml` and `Cargo.lock`, and atomically pushes that commit with the immutable `cadence-nightly-<run-number>-<12-character-original-commit>` tag. The nightly version is the reserved base plus `-nightly.<run-number>` (for example, `0.1.1-nightly.6`), and reruns reuse the same reservation, source commit, build id, tag, and artifact. The reservation tag is verified when the GitHub prerelease is published. A tag-triggered nightly may instead use an exact tag such as `v0.1.0-nightly.1`; explicit stable, RC, and tag-triggered nightly releases do not reserve or automatically bump the package version.

Release versions are channel-specific: stable is `X.Y.Z`, RC is `X.Y.Z-rc.N`, and nightly is `X.Y.Z-nightly.N`. Automatic nightlies use one globally increasing numeric patch stream across all three channels; existing nightlies are never renumbered. The full semantic version remains in the artifact name and manifest. Apple bundle metadata is separate: `CFBundleShortVersionString` is the numeric base `X.Y.Z`, while `CFBundleVersion` is the numeric build value (`X.Y.Z` for stable and `N` for RC/nightly). A failed or canceled run before the reservation consumes no patch; once the reservation commit and tag are pushed, later retries reuse that patch.

Both the build and publish jobs require the fixed `cadence-production` GitHub Actions environment. Configure that environment externally before a production release: allow only protected `main` and matching release tags, and require the reviewers selected in the environment protection rules. The existing manual stable-source guard remains defense in depth; it still accepts only `main` or the matching `vX.Y.Z` tag. Nightly and push-tag triggers retain their existing behavior.

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

The native app owns its library model and JSON persistence in the Cadence host, uses Radiant’s typed native file picker and file-drop boundary, decodes imported files off the UI thread, renders a retained waveform plus timestamped comments, and provides host-controlled audition playback.

## First slice

- Import audio through the button or by dragging files onto the native workspace.
- Drop multiple audio files onto the native workspace to queue them for serial import; each new track starts in Inbox.
- Filter the local library by status, including All.
- Mark tracks as favorites with a persistent star toggle.
- Move tracks through Sound design, Production / arrangement, Mixdown, and Mastering stages.
- Move any track directly between the independent statuses Inbox, Refine, Release, Archive, and Maybe. Maybe captures an uncertain decision; Archive is a visible, reversible marker; Favorite remains a separate star.
- Remove imported tracks from the library; native removal keeps the external source audio file in place.
- Switch to the native finishing board: four columns derived from each track's current stage, with cards for review, favorites, and comment counts. Drag cards between columns to update their workflow stage.
- Play, pause, seek, and adjust native audition volume with the transport controls; the LUFS meter reports K-weighted integrated LUFS from decoded audio.
- Switch directly between the visible Review, Planner, and Audition tabs in the native workspace header. Audition filters the library by Inbox, Refine, Release, Archive, or Maybe, fixes a shuffled one-pass queue, and advances to the next matching track automatically; status changes made while listening update the queue.
- Import one external reference track per native track; its independently decoded waveform is shown below the primary waveform at the same height without changing loudness analysis.
- Play the imported and reference tracks from one synchronized transport, then use the compact icon source toggle to choose the audible source.
- Drag across the reference waveform to paint a normalized loop range; the shared transport keeps both tracks synchronized and repeats the selected section.
- Toggle loudness matching to apply the bounded LUFS-derived gain offset to the reference audition.
- Drag the upper half of the shared waveform to scrub, then release to play from that point.
- Use the lower half of the same waveform to open an inline comment composer at the exact hovered/clicked timestamp; saved comments appear as dots on the horizontal comment line.
- Edit saved comments in place without changing their timestamp.
- View the full-track integrated LUFS value beside the native waveform; it remains stable while stopped or playing.
- Press `N` or click the lower comment rail to capture the current position and write a note.
- Click a comment pin or note timestamp to return to that moment; saved comments can be selected, played, edited, or deleted.

The native planner is currently a single derived board over the library's four progress stages. Dragging a card between columns updates the existing persisted track stage; the independent track status remains available from each library row, planner card, and selected-track header.

## Known limits

### Native app

The native app persists library metadata and the original external audio-file paths as JSON under the Cadence application-support directory, then decodes the source file in a background worker to build a bounded retained waveform summary. Moving or deleting a source file therefore requires re-importing it. The native launcher takes a single-process lock around that local library.

Native playback is responsive, host-controlled audition playback through Rodio. The playhead displays the latest Rodio-reported position at Radiant frame cadence; it is not a sample-accurate transport clock or a lock-free realtime audio engine. Rodio/CPAL may pull decoder data and service internal control state from the output callback, so occasional device-, decoder-, or system-load-related glitches remain possible. A future DSP, recording, monitoring, plugin-hosting, automation, low-latency scrubbing, or sample-accurate transport requirement would need a dedicated callback-safe backend. Native loudness uses bounded K-weighted integrated LUFS analysis decoded in the background; playback gain is applied after that analysis, so audition volume and reference matching do not change the meter. Reference tracks are stored as external paths and must be re-imported if moved or deleted; matching changes only reference audition gain and never rewrites either audio file. The native planner supports drag-to-stage movement but does not yet persist custom boards; track statuses are stored separately from production stages.
