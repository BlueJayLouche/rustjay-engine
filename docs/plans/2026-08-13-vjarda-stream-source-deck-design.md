# Vjarda Stream Source Deck Design

## Goal

Make network streams a first-class Deck tab source while retaining the Effects/Library entry point and rejecting malformed or unsupported URLs before a deck is queued.

## Design

Keep URL classification in the source registry, next to `SourceKind` and `streams.txt` parsing. A small standard-library classifier will allow `srt`, `rtmp`, `rtmps`, `http`, `https`, and `rtsp`, require a non-empty host, and preserve the existing HLS/DASH kinds when an HTTP path has a `.m3u8` or `.mpd` extension. This avoids a dependency and keeps every caller consistent.

Add `Http` and `Rtsp` to `SourceKind`; both follow the existing ffmpeg-backed `StreamSource` instantiation branch. Invalid `streams.txt` entries are logged and skipped.

The Deck tab gains Stream state and a Stream selector. With ffmpeg enabled, its form accepts a URL and optional display name, paints classifier errors inline, and queues a valid stream. With ffmpeg disabled, the selector remains visible but disabled and explains the feature requirement on hover.

Both Deck and Effects forms call one UI helper that classifies the URL, builds the `SourceEntry`, queues the `PendingDeck`, and emits the notification. A blank display name falls back to the trimmed URL. The Effects form drops its manual kind selector because kind is inferred from the URL.

## Verification

- Unit-test every accepted scheme, HLS/DASH HTTP suffix classification, and missing/unknown scheme or host rejection.
- Extend the Deck kittest snapshot assertion for the Stream option.
- With ffmpeg enabled, interact with the Deck Stream form and assert an invalid URL paints its inline error.
- Refresh the default snapshot and run the requested workspace checks, clippy, vjarda tests, projection tests, and ffmpeg check.

## Implementation checklist

1. Add and test the registry classifier; route `streams.txt` through it.
2. Extend exhaustive `SourceKind` matches and ffmpeg stream instantiation.
3. Add the shared queue helper and update both UI entry points.
4. Add the feature-gated Deck selector/form behavior.
5. Update kittest coverage and snapshots, then run all gates.
