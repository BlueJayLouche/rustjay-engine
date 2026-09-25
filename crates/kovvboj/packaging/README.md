Icon slot for release packaging (picked up by .github/workflows/release-apps.yml):

- `AppIcon.icns` — macOS bundle icon
- `icon.ico` — Windows Start-menu shortcut icon

Both are generated from `icon.svg` (Workbench KVJ outlined to paths — no font needed):

```sh
mkdir AppIcon.iconset && for s in 16 32 128 256 512; do
  resvg -w $s icon.svg AppIcon.iconset/icon_${s}x${s}.png
  resvg -w $((s*2)) icon.svg AppIcon.iconset/icon_${s}x${s}@2x.png; done
iconutil -c icns AppIcon.iconset && rm -r AppIcon.iconset
python3 -c "from PIL import Image; Image.open('icon.png').save('icon.ico', sizes=[(16,16),(32,32),(48,48),(256,256)])"  # after: resvg -w 1024 icon.svg icon.png
```
