# Cadence

Cadence is a local-first track review desk: import audio, listen, capture a note at the current time, and check it off when the next pass is done. The Rust/Radiant native app is the root project; the dependency-free browser prototype lives in `web/` as a separate local preview.

## Run locally

On macOS, double-click [`Cadence.command`](./Cadence.command) to start the browser prototype from `web/`. The native Radiant app can be launched from the project root with `cargo run --release` or the compiled `target/release/cadence-native` binary; it opens a desktop window and stores its native library metadata under the normal Cadence application-support directory.

To run it from Terminal:

```sh
./Cadence.command
```

The launcher normally opens the browser for you. To start the server without opening a browser, use `CADENCE_NO_OPEN=1 ./Cadence.command`.

If that port is already occupied, choose another free port and open the matching URL. The launcher serves the dedicated `web/` directory automatically; port `4173` may belong to another local PortalSurfer site.

The browser app stores track metadata and review notes in `localStorage`, the imported audio blobs in IndexedDB, and the last browser session in a separate local storage record. Nothing is uploaded anywhere. Refreshing the page restores the selected track, listening position, playback intent when the browser permits autoplay, audition volume/mute state, search, and review filters. The bundled “Glass Echoes” entry is a local preview record used to make the review layout inspectable before importing a real file.

The native slice owns its library model and JSON persistence in the Cadence host, uses Radiant’s typed native file picker and file-drop boundary, decodes imported files off the UI thread, renders a retained waveform plus timestamped comments, and provides host-controlled audition playback while the browser prototype remains available.

## First slice

- Import audio through the button or by dragging files onto the page.
- Search and filter the local library.
- Mark tracks as favorites with a persistent star toggle.
- Move tracks through Sound design, Production / arrangement, Mixdown, and Mastering stages.
- Remove imported tracks from the library; native removal keeps the external source audio file in place.
- Switch to the native finishing board: four columns derived from each track's current stage, with cards for review, favorites, and open comments. Drag cards between columns to update their workflow stage.
- Play, pause, and seek with the native transport controls; browser-only audition volume remains separate.
- Drag the upper half of the shared waveform to scrub, then release to play from that point.
- Use the lower half of the same waveform to open an inline comment composer at the exact hovered/clicked timestamp; saved comments appear as dots on the horizontal comment line.
- Edit saved comments in place without changing their timestamp or completion state.
- Watch the far-right K-weighted loudness meter while listening: 3-second short-term, live integrated, and full-file integrated readings are shown together. The live readings accumulate before the audition gain, so volume and mute do not affect them. The -7 to -6 LUFS hard-techno reference is a practical mix/master heuristic, not a universal delivery standard.
- Press `N` or click the lower comment rail to capture the current position and write a note.
- Click a comment pin or note timestamp to return to that moment; check notes off as they are completed.
- Use Open, All, and Done note views.

The native planner is currently a single derived board over the library's four progress stages. Dragging a card between columns updates the existing persisted track stage; custom boards and board-specific status remain future slices.

## Known limits

### Browser prototype

The browser prototype relies on the browser’s native audio codecs and local browser storage. Imported files are decoded locally to produce their waveform peak envelope and integrated loudness using the 48 kHz ITU-R BS.1770 K-weighting coefficients, per-channel energy summation, 400 ms blocks, and absolute/relative gating. The live meter uses the same per-channel path before the audition gain. It does not yet include server persistence, bulk folder watching, collaboration, or the planner itself. Clearing browser site data removes the browser library.

### Native Radiant slice

The native slice persists library metadata and the original external audio-file paths as JSON under the Cadence application-support directory, then decodes the source file in a background worker to build a bounded retained waveform summary. Moving or deleting a source file therefore requires re-importing it. The native launcher takes a single-process lock around that local library, while the browser library remains separate.

Native playback is responsive, host-controlled audition playback through Rodio. The playhead displays the latest Rodio-reported position at Radiant frame cadence; it is not a sample-accurate transport clock or a lock-free realtime audio engine. Rodio/CPAL may pull decoder data and service internal control state from the output callback, so occasional device-, decoder-, or system-load-related glitches remain possible. A future DSP, recording, monitoring, plugin-hosting, automation, low-latency scrubbing, or sample-accurate transport requirement would need a dedicated callback-safe backend. Native loudness metering remains a separate slice; the native planner supports drag-to-stage movement but does not yet persist custom boards or board-specific status.
