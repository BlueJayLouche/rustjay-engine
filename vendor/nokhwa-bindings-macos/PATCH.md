# Local patches: `nokhwa-bindings-macos` 0.2.4

**One file differs from the published crate: `src/lib.rs`.** Everything else is
pristine.

All of the changes are in `AVCaptureDevice::set_all()`, and all of them exist
because USB capture devices (and Continuity cameras — anything reporting as
`AVCaptureDeviceTypeExternal`) expose format lists that Apple's own cameras do
not, and AVFoundation responds to a bad configuration by *raising an
NSException*, which crosses the FFI boundary as an uncatchable panic.

## The patches, in the order they were made

1. **`setActiveFormat:` instead of KVC** (`c3936c1`). `activeFormat` is
   read-only via KVC on modern macOS; `setValue:forKey:` raises.
2. **KVC restored for the frame durations** (`5e18b1b`). `minFrameDuration`
   returns a `CMTime` *struct*, not an object pointer. Storing it in
   `*mut Object` corrupted it and crashed with EXC_BAD_ACCESS. KVC is correct
   here because it boxes `CMTime` in an `NSValue` automatically.
3. **Format and frame-rate range picked together** (this patch). See below.

## Why (3) was needed

The selection loop assigned `selected_format` for *every* format whose
resolution matched, but only assigned `selected_range` when a frame rate also
matched — and never cleared it. There was no `break` on the outer loop, so a
later resolution-match could overwrite the format while leaving a range that
belonged to an earlier one. `activeFormat` and `activeVideoMinFrameDuration`
then described different formats, and AVFoundation raised
`NSInvalidArgumentException`.

Apple's cameras rarely trip this: they list each resolution once. USB capture
dongles routinely list one resolution several times, and Continuity cameras
list each resolution at both 30 and 60.

Confirmed against the reporting hardware, an "AV TO USB2.0" dongle, which
lists every resolution twice — `yuvs` and `420v`, with different frame-rate
sets. **11 of its 13 advertised format entries produced a mismatched pair**
under the old logic; 0 do now. Its 720x576 pair is the PAL case:

    [ 8]  720x576  420v  maxFps: 60, 50, 30, 10
    [ 9]  720x576  yuvs  maxFps: 25, 20, 10, 5

Asking for 576p50 took the range from `[8]` but ended up activating `[9]`,
which tops out at 25fps. That table is pinned as a regression test in
`mod selection_tests`.

The picking now lives in `select_format_and_range()`, a pure function over
`(width, height, [max frame rate])`, which returns both halves together or
nothing. It is unit-tested in `mod selection_tests` — a seam that exists
precisely so this cannot regress a fourth time without a test going red.

Two further defects were fixed alongside it, both in the same function:

- `if !accepted == YES` never fired. `!` is bitwise-NOT on `BOOL` (i8), giving
  -1 for `NO` and -2 for `YES`, so a **rejected `lockForConfiguration` fell
  straight through into the setters**. Now `if accepted != YES`.
- The "format not found" early return left the device **locked for
  configuration**, so every later attempt to configure it failed. It now
  unlocks first.

## Checking whether this is still needed

`select_format_and_range` is ours; upstream nokhwa 0.2.4 has no equivalent.
Re-check on any nokhwa bump, and carry all three patches forward.

There is a read-only AVFoundation probe that dumps every device's formats and
replays the selection, flagging pairs that would raise — useful when a new
device misbehaves. See the PR that introduced this patch.
