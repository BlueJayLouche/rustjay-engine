# CuePool ASIO output design

## Goal

Make CuePool's existing `audio_output_driver` and `audio_output_device` settings control the CPAL host and device used for playback. WASAPI remains the default. A project requesting ASIO must never emit audio through WASAPI.

## Chosen design

`cuepool-audio` owns audio-backend policy. It exposes one driver-aware host selector, and engine creation plus device enumeration both use that selector. The crate's optional `asio` feature forwards to `cpal/asio`; the CuePool binary exposes and forwards the same feature. ASIO host construction is compiled only for Windows with that feature enabled. Without the feature, an ASIO request produces a deliberate `ASIO support was not compiled in` runtime error. With the feature enabled off Windows, it reports that ASIO is Windows-only. This allows the decision table and messages to be tested on Linux without pulling in the SDK.

The app applies the current show's driver/device configuration at startup, after a new project or show file is loaded, after show settings are imported, and when the operator changes either setting. The settings window gains a driver selector. Changing the driver refreshes its device list and opens its configured/default device; changing the device opens that exact named device. A successful choice is written back to the existing schema fields and marked dirty.

Audio changes use a dedicated application command. `project_generation` remains limited to whole-project and projection/lighting resets, so importing show settings or restoring them through undo/redo cannot close projector windows or reset lighting. The audio engine owns its active driver and device identity; applying unchanged settings is a no-op when the engine is healthy. Host creation, device enumeration, and engine construction share one configuration attempt.

No schema or migration changes are needed: both fields already use serde defaults, so old projects continue to deserialize as WASAPI with an empty device name. Tests will explicitly preserve that compatibility and round-trip an ASIO driver/device pair.

## Failure behavior

Configured-device lookup is exact. Errors name the requested driver and device and, when enumeration succeeded, list available device names. Host unavailability, enumeration failure, unsupported stream configuration, stream construction, and stream start failures retain that context.

If a project requests ASIO and selection or stream startup fails, CuePool stops existing cues, drops the current engine/stream, keeps the project open, disables audio cue playback, and publishes the error to the settings UI and log. It never falls back to the default host or an unnamed device. Selecting a valid output later re-enables playback.

For the default WASAPI configuration with no persisted device name, CuePool selects that host's default output device, preserving current behavior. A non-empty configured device is always authoritative for every driver.

## Stream and routing

Stream selection still prefers F32, eight output channels, and 48 kHz. It also accepts I32 and I16 because Dante Virtual Soundcard can expose native integer ASIO formats; samples are converted from the mixer's existing F32 bus in the output callback. Per-cue routing remains in the existing mixer, which receives the actual stream channel count.

Integer conversion scratch space is allocated before stream construction from CPAL's advertised maximum buffer size. Hosts without an advertised maximum use a fixed generous cap; an unexpectedly larger callback is silenced rather than allocating on the realtime thread.

## Verification

Linux tests cover the host-independent driver decision table, ASIO-disabled error text, missing named-device formatting (including alternatives), old-project defaults, and ASIO driver/device serialization. Default-feature checks and targeted `cuepool-audio`/`cuepool-core` tests run locally. Enabling CuePool's `asio` feature on Linux is intentionally a no-op at dependency-build level and produces the same runtime unsupported-platform error if selected.

Windows verification must build `cuepool --features asio` with the Steinberg ASIO SDK (`CPAL_ASIO_DIR`) and LLVM/libclang available, enumerate `Dante Virtual Soundcard (x64)`, open an eight-channel 48 kHz stream, verify all eight routing outputs, and exercise missing-driver/open-failure messages without WASAPI output.

## Implementation plan

1. Add and forward the opt-in Cargo feature with target-specific CPAL activation.
2. Centralize driver-to-host selection, device enumeration/lookup, contextual errors, and configured engine construction in `cuepool-audio`.
3. Make the app's audio engine optional so failure closes the output stream; apply show settings on startup, project changes, and settings commands.
4. Add the driver control, persisted device updates, and visible disabled/error state to the settings UI.
5. Add portable unit and serialization tests, update the audio guide, run formatting/checks/tests, and review the final diff.
