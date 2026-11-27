# Changelog

All notable Cadence releases are documented here.

## Unreleased

## 0.2.0

- Group dropped `<BATCH><SLOT>_b<BUILD>[__<track-title>]` bounces by batch + slot (uppercase batches `A–Z`, `AA`…, slot `0–9`, lowercase `b` and legacy uppercase `B`, zero-padded build normalization, and untitled files) with highest numeric build current regardless of drop order, without renaming source audio on disk.
- Ignore duplicate re-drops, flag conflicting audio for an existing build as a non-destructive collision (`Keep existing`, `Replace bN`, or `Separate track`), and support manual assignment (`+ Bounce` and `Assign to track`) for non-matching files.
- Keep workflow stage and a persisted `next_action` attached to the track across new builds while binding timestamped notes and optional build feedback notes (e.g. `"Heard by Dylan"`) to each bounce revision; support switching to prior builds and in-place relinking of moved audio files without losing notes or duplicating cards.
- Upgrade Planner cards to display current bounce, stage, one clickable next action, and missing/collision status while consolidating redundant per-card controls; add a compact Review context and bounce revision strip.
- Redesign UI with a modern terminal black/grey/white/signal-red palette, make entire track cards clickable, move the waveform comment rail to the bottom third with darkened reflection and hover cursors, and switch to GPUI's native macOS Metal renderer (`runtime_shaders`) to fix multi-monitor window-move crashes.
- Add a Reference Manager tab with import, search, rename, file replacement and removal, including assignment counts and imports before a main track is selected.

- Compute spectrogram colors and time/frequency placement on Metal, then draw shared IOSurface textures directly through GPUI; bound queued work and buffers, retain paused frames, and keep a CPU fallback.
- Add a black/purple/red/yellow spectrogram heatmap, high-to-low frequency axis and left-to-right time history with a quiet/loud legend.

- Reuse waveform and loudness analysis from a persistent disk cache and bounded memory cache, including immediately after import; keep source verification and cancellation checks.

- Replace the blocking comment modal with a small editor beneath the note rail; drag new or existing markers while typing and save their updated position.

- Tighten waveform bars from 6px/12px to 5px/10px for more visible detail; retain quiet dots and rounded ends.
- Keep Track Manager selection checkmarks visible while their checkbox has focus.

- Integrate the recovered Frame Technical Futurist GPUI redesign: Review, four-stage Planner, persistent colored task tags and album groups, and Track Manager bulk edits.
- Migrate existing seven-stage values to Backlog, Production, Mixdown and Mastering while retaining tracks and selection.
- Add keyboard and modal navigation, multiline timestamped comments, paired LUFS meters, waveform loops and source controls.
- Retain invalid comment drafts, flush queued library work before quitting, and support an isolated review data directory.
- Keep the optional Radiant comparison binary and existing native macOS bundle tooling.
