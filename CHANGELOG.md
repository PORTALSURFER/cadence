# Changelog

All notable Cadence releases are documented here.

## Unreleased

- The Planner board now stays within the available width so all seven stages can be reached by scrolling. Moving a card to another stage automatically reveals its destination column.
- Waveforms now show a pixel-aligned hover guide and exact comment time, use a darker lower half, and cache their drawn geometry for responsive hovering. Escape pauses playback.
- The Review view now has a SoundCloud-style center comment line: click above it to play, or press below it and slide immediately to place a comment draft. Draft and saved nodes can also be hovered and dragged later. Drag tracking continues across the window. A persistent composer and list sit below the waveforms. Reference selection has moved into a toolbar dropdown that contains wheel scrolling.
- The GPUI workspace now follows the Freqs device theme with coral section labels, dark bordered panels, compact controls, and matching waveform and input colors.
- The native Cadence workspace now uses GPUI for track review, waveform and note interaction, reference management, transport controls, and the Planner. The former Radiant app remains an optional comparison binary.
- Native macOS app bundle and signed nightly release pipeline are now available.
- Planner is now the single workflow board, using Backlog, Groove, Arrangement, Polish, Mixdown, Master, and Release stages; legacy status fields are ignored on load and omitted on save.
