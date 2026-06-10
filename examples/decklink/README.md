# DeckLink Input Example

Captures video from a Blackmagic Design DeckLink device and renders it through
rustjay-engine. The capture path uses the DeckLink **COM API** via a small C++
wrapper (`src/decklink_sdk_wrapper.cpp`), compiled with `cc` and linked against
`ole32`/`oleaut32`.

**Windows-only.** On macOS/Linux the example still builds (so `cargo build
--workspace` stays green) but does nothing — the capture code is `#[cfg(windows)]`.

## Build requirements (Windows)

- **Blackmagic DeckLink drivers** installed.
- The **DeckLink SDK** — you must supply its header (see below).
- A C++ toolchain (MSVC).

### Supply the SDK header

`DeckLinkAPI.h` is Blackmagic's proprietary SDK header and **cannot be
redistributed**, so it is **git-ignored** and not included in this repo. Before
building on Windows, copy it from your installed SDK into the example's `src/`:

```
copy "C:\Path\To\Blackmagic DeckLink SDK\Win\include\DeckLinkAPI.h" ^
     examples\decklink\src\DeckLinkAPI.h
```

(The build script prints this reminder and fails fast if the header is missing.)

## Running

```bash
cargo run -p decklink-input    # on Windows, with DeckLinkAPI.h in place
```

The example opens the first DeckLink device (`device_index = 0`) in
`DecklinkSource::new`. Change the index if you have multiple cards.
