Icon slot for release packaging (picked up by .github/workflows/release-apps.yml):

- `AppIcon.icns` — macOS bundle icon
- `icon.ico` — Windows Start-menu shortcut icon
- `icon-256.png` — runtime window/taskbar icon (Windows + X11), embedded by `src/main.rs`

All generated, with `icon.svg`, by `uv run .github/packaging/app_icon.py <app>` — edit the design there.
