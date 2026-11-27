# Recovered Frame redesign acceptance

Source: original cloud chat `CLOUD - CADENCE`, ID `01a0f225-38fb-774b-ae05-42f2a0a2c065`. Local integration baseline: `93fee415e22958a66dc298ab113ed34009739aa9`. Preserve later local fixes and user libraries. No commit, push, publication or merge is authorized.

- [x] Recover the approved editable Frame source, including the final comment-source icons.
- [x] Integrate native source with local GPUI, storage, audio and transport behavior.
- [x] Match Technical Futurist composition: 32px margin, 8px grid, charcoal surfaces, rails, square controls, monospace captions, orange selection, shared compact cards.
- [x] Verify Review main/reference selection, matching, volume, two LUFS meters, aligned waveform/rulers, seek/loop interactions, timestamped comments and source icons.
- [x] Verify Planner stages Backlog, Production, Mixdown and Mastering.
- [x] Verify persistent task/album groups, tags, colors, editing, creation, search and matching track previews.
- [x] Verify Track Manager import, search, sorting, filters, selection, staged persistent bulk stage/tag/group edits and meaningful errors.
- [x] Verify small-window layout, crowded labels, bulk panel overflow, picker arrows and tooltips.
- [x] Verify visible focus, keyboard activation, modal containment/restoration, inline errors, keyboard volume and one-second seek.
- [x] Run meaningful Rust tests, strict Clippy, formatting, build and diff checks.
- [x] Build a fresh macOS application and record exact source, bundle/signature/hash evidence and screenshots where feasible.

Dependencies: native source recovery and careful comparison with the local implementation precede integration. Historical cloud test results are not current local verification. Hardware audio, DAW acceptance and screen-reader behavior require separate evidence.

## Approved local integration decision

The user selected the cloud four-stage model. Saved Groove, Arrangement and Polish stages migrate to Production; Master and Release migrate to Mastering. Existing Sound Design values retain the Backlog label. An automated library round-trip covers all seven prior values.

`CADENCE_DATA_DIR=/absolute/path/to/disposable-review-library` isolates an application run from the normal macOS Application Support library.

## Current local evidence

Validation on 2026-10-03: `cargo test --locked --all-features --all-targets` passed 186 native and 602 legacy tests; strict all-target/all-feature Clippy, formatting, debug build and diff whitespace checks passed. The tests cover source identity, retained waveform summaries, transport acknowledgements, storage durability, four-stage migration, persistent tag integrity, atomic bulk edits, stale selections and disconnected-worker shutdown.

Native macOS UI checks used a disposable `CADENCE_DATA_DIR` and generated ten-second WAV fixtures. Verified real import and reference decode, paired LUFS (-23.7/-23.6), visible waveform bars/rulers, multiline comment persistence, empty-comment inline validation and draft retention, keyboard Tab/Return save, colored task-tag creation, album creation into Albums, staged Production/tag apply, assigned-track preview, four Planner columns and visible saved cards, restart persistence, keyboard Home volume and one-second Right seek while muted, and matching activation. Review and Tags were checked near the minimum width; Review scrolls to reveal comments. Browser checks rendered all six recovered Frame screens without console errors.

The acceptance checklist records integration and scoped automated/UI checks, not exhaustive hardware acceptance. Audible playback, extended loop audition, large-library navigation, crowded-label stress and screen-reader acceptance remain manual checks. No active DAW session was operated. Computer Use could read only window-level accessibility nodes from GPUI; screen-reader support is unverified.

Native acceptance caught and repaired missing Planner/assigned-card list height, tag editor save controls positioned relative to the color strip, and collapsed comment preview width. A failed comment save retains its draft; successful validation clears the old inline error. Application quit waits for the serialized worker queue.

Source recovery used actual recorded cloud heredocs and source mutations. Editable Frame source is traceable to its recorded revisions; the native source includes local integration repairs. The cloud's Clippy auto-fix had no recorded final diff, so native byte identity to that cloud checkout is not claimed. Local audio, source-ticket, transport and signal-summary modules were retained. Dependencies were resolved and locked locally.

The source and review artifacts are also copied into `/Users/portalsurfer/dev/cadence`. The existing untracked reference photo is preserved. No commit or push was made. The review application is an ad-hoc-signed debug build, not a published release.

## Waveform and checkbox refinement, 2026-10-03

Waveform bars now use 5px width, 10px pitch and 2.5px pill radius, with 5px quiet dots. This provides approximately 20 percent more displayed peak buckets at the same width. The editable Frame preview uses the same density.

Track Manager checkbox focus now retains the selected orange fill and a visible focus ring. The generic dark focus background had hidden the dark checkmark while the checkbox was focused. Native UI verification covered mouse selection, keyboard Return selection/deselection with focus retained, and immediate selected counts. The checks used an isolated app identity and disposable library; the user's running Cadence was preserved.

The fresh review bundle for this refinement is `dist/frame-refinement/Cadence.app`. Reopen this bundle to use the new executable.

## Analysis cache and interactive comment editor, 2026-10-03

Acceptance scope: reuse unchanged analyses in memory and after restart, retain waveform Arc identity, reject changed/missing sources, tolerate corrupt cache entries, preserve cancellation and selection epochs, and avoid repeat analysis immediately after import. First-time analysis remains necessary. The existing bounded waveform cache format and source-ticket checks remain the authority.

Comments use a 300px floating editor anchored beneath the marker, without a full-window backdrop or a modal focus trap. The rail previews new markers and dragged positions; Save commits text and time together, and Escape discards the draft. The editor follows horizontal movement and keeps typed text while the rest of Review remains interactive. Existing comments are re-sorted after retiming.

Native checks so far: typed a new comment, dragged it from about two seconds to six seconds with the editor open, toggled Match from the toolbar while the draft remained open, and saved the correct text and position in the disposable library. Final build and automated verification are recorded in the artifact manifest.

Final validation: 189 native and 603 legacy tests passed (792 total), strict all-target/all-feature Clippy passed, and the final GUI focus adjustment passed native Clippy again. Formatting, whitespace, debug build and deep/strict ad-hoc signature verification passed. The fresh bundle is `dist/cache-comment/Cadence.app`.

The final native check also reopened a saved comment, dragged it from six to eight seconds, continued typing after drag release, saved, and verified both text and 8,333ms in JSON. Escape cancelled a new draft. A short-window reference editor stayed usable by flipping above the rail when insufficient space remained below; the marker rail remained exposed. Checks used disposable data and an isolated bundle identity. Persistent cache entries loaded with both main/reference LUFS present after restart; automated cache tests prove the hit bypasses audio decoding even for an otherwise undecodable source fixture. Cache hits still perform source checks and I/O; first-time analysis and playback decoding are separate work.

## Live spectrogram coloring and performance, 2026-10-03

Acceptance scope: conventional magnitude coloring, responsive native rendering, unchanged audio analysis/playback semantics, bounded CPU/GPU resources, image reuse on duplicate or paused frames, correct time/frequency orientation, and invalidation when the source/session changes. Prior source/cache/comment changes and the reference photo remain intact.

The native Spectrogram replaces the prior Waterfall mode's per-cell translucent orange drawing. An inferno-style black/purple/red/orange/yellow-white palette is interpolated once into a 256-entry BGRA lookup table. The frequency axis runs from high at the top to low at the bottom; time runs left to right with newest audio at the right. History retains a fixed duration during initial filling. A quiet/loud legend and frequency/time labels explain the display.

At full history, the old image used up to 240 × 128 = 30,720 separate cell quads on every UI paint. The new renderer builds one bounded 240 × 128 texture only when the globally unique live-frame revision changes and paints one image. Playhead ticks, resizing and pause reuse the cached image. Old textures are explicitly removed from GPUI's sprite atlas; no unbounded stream of image IDs remains cached. FFT, capture, analysis cadence, loudness, matching and source-ticket behavior are unchanged. The transport hop constant is exposed to the native display solely to label history duration.

Automated checks: 191 native and 603 legacy tests passed (794 total); strict all-feature/all-target Clippy, formatting and whitespace checks passed. New tests cover time direction, frequency direction, opaque BGRA magnitude pixels, duplicate-frame image identity, retirement on a new session with equal local counters, and clearing unavailable frames.

Native comparison used isolated app identities, disposable libraries and the same generated 32-second 48kHz tone/noise fixture, muted playback, debug arm64 builds and a 1440 × 900 window. An independent process CPU-time sampler measured eight playback seconds after a three-second start/history warmup. Baseline used 8.48 CPU seconds (105.96% average CPU); updated used 3.87 CPU seconds (48.35%), a 54.37% reduction. This is one local process CPU comparison, not an FPS guarantee or audible acceptance. The native screenshot verified multicolor intensity, horizontal time flow and full history. The automation could retrieve the new app's window state promptly while the old renderer stalled window-state retrieval during playback.

Fresh review bundle: `dist/spectrogram-review/Cadence.app`. The signed debug bundle, fixture method, CPU comparison and fresh source hashes are recorded in `artifacts/native-review`. No commit or push was made.


## Direct Metal spectrogram follow-up, 2026-10-03

The follow-up request is to reduce the remaining load and use the GPU directly. Acceptance covers real Metal compute dispatch, direct GPUI IOSurface drawing, no GPU wait on the UI/audio thread, bounded buffers and latest-only work, stale source/session rejection, pause reuse, color/orientation parity, fallback on unsupported hardware, and a measured native comparison. The optimized bundle must be built from the final source in `/Users/portalsurfer/dev/cadence`.

A new profile of the previous CPU-image debug app measured 3.77 CPU seconds over 8.01 wall seconds (47.06% average process CPU) on the same muted fixture. Sampling showed substantial GPUI layout/paint cost; the palette conversion and FFT were smaller contributors. An optimized build is part of this acceptance so compiler overhead is measured separately from the GPU change. The measurements below separate the GPU renderer from the optimized build.


The macOS renderer now uploads at most 30,720 magnitude bytes per new frame. A Metal compute kernel maps chronology, frequency orientation and magnitude colors into a 480 × 256 NV12 IOSurface (each analysis cell occupies 2 × 2 pixels to preserve chroma). GPUI imports and draws that completed surface directly; the UI does not perform the per-pixel BGRA conversion or atlas upload on the GPU path. FFT analysis and audio capture remain on their existing CPU worker. A cached CPU image handles startup, unsupported hardware and initialization/render errors.

The dedicated Metal worker owns its device, queue, pipeline and texture cache. Pending work and completed output each occupy one latest-only slot. The CoreVideo pool permits at most four buffers and applies worker-only retry under pressure; the UI never waits for Metal or worker shutdown. Published buffers remain immutable until GPUI releases them. Source, generation and epoch fences reject stale completions, while a 16ms completion poll updates the last frame even after pause. New revisions retain the previous completed surface until their replacement is ready.

Actual Metal readback tests pass after applying GPUI's YUV-to-RGB matrix, with a maximum allowed four-level channel quantization tolerance. Tests cover chronology/frequency orientation, the brightest palette endpoint, paused completion repaint, duplicate frame reuse, source fencing, retained buffer immutability, pool allocation limits and recovery after releasing retained buffers. The available CoreVideo wrapper adopted a borrowed Metal texture as owned; Cadence explicitly retains the borrowed texture and uses the native borrowed-device ABI to keep teardown ownership balanced.

Final automated validation: 194 native and 603 legacy tests passed (797 total), strict all-feature/all-target Clippy passed, formatting and whitespace checks passed, and debug plus optimized release builds passed. The fresh release bundle is `dist/gpu-spectrogram/Cadence.app`; deep/strict ad-hoc signature verification passed.


Native comparison on Apple M5 Pro used the same muted 32-second mono 48kHz fixture and approximately 1440 × 900 window, measuring eight playback seconds after a three-second warmup. Previous CPU-image debug: 47.06% average process CPU; GPU debug: 45.33%; delivered GPU release: 15.23%. The GPU-only debug change reduced CPU by 3.67%; the combined renderer and optimized build reduced it by 67.63%. Compiler optimization is the larger contributor here; these are single local process CPU measurements, not FPS guarantees. The baseline was also sampled to locate hot paths, and GPU diagnostics were enabled in both GPU runs.

Native GPU logs recorded 974 completed debug dispatches and 1,027 completed release dispatches with no initialization/render errors. Screenshots confirmed the conventional multicolor heatmap, filled history and fresh history after restart. Pausing held the completed frame and produced no additional GPU dispatches over a two-second check; Stop cleared history. Playback checks used disposable libraries and isolated app identities, with no active DAW session operated. Final hashes and measurements are in `artifacts/native-review/artifact.json` and `gpu-performance.json`. No commit or push was made.


## Reference Manager tab, 2026-10-03

Acceptance scope: a visible Reference Manager tab; catalog imports without a main track; reference names, file paths and assignment counts; replacing files through the existing source-proof authority; confirmed catalog removal with source audio retained; search/empty states; drag-and-drop routed to the reference catalog in this tab; usable normal and small-window navigation; and persisted operations after restart. Keep the Review reference selector and all prior GPU/cache/comment/Planner changes. Automated verification, a fresh optimized bundle and native checks use a disposable library; final evidence follows below.

Native verification used generated WAV files and a disposable library. Imported a reference with no main track, renamed it to Studio Reference, replaced its audio, checked the no-match search state, and opened/canceled the removal confirmation. Restarting with the persisted library retained the replacement path, display name and source proof. Imported a main track, assigned the reference through Review, and verified the manager displayed one assignment and zero unassigned files. Screenshots cover the empty catalog, import, rename, replacement, search, confirmation, restart and compact layout. Review controls now occupy a second header row below 1600px to prevent overlap with the fifth tab; native checks covered normal and approximately 1040px-wide compact windows. Actual removal and same-file re-import semantics are covered by existing storage tests; no user audio or library was changed. Drag-and-drop routing was inspected in code, not exercised through desktop automation.

Validation: 194 native and 603 legacy tests passed (797 total), strict all-feature/all-target Clippy and formatting passed, and the optimized release build plus deep/strict ad-hoc signature verification passed. The final bundle is `dist/reference-manager/Cadence.app`. This feature changes only `src/gui.rs` and the README, changelog and acceptance record; all earlier source changes and the unrelated reference photo were preserved. The scoped source and evidence are synchronized with the attached redesign worktree. No commit or push was made.
