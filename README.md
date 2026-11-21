# Skin Studio

An osu! followpoint animation studio with a live gameplay preview, alongside an in-progress skin editor. Right now the followpoint animator is the main, most complete part of the tool; the broader skin editor (image filters, restoration, artifact scanner, `skin.ini`, and element creator) is functional and actively growing toward a full osu! skin suite.

## What it does

- **Followpoint animator**: effect presets, drawn shapes or your own PNG, timing, keyframe motion and color, outline, shadow, and debanding. Export as `followpoint-N.png` (`@2x` and `1x`), import an existing set, and browse your save history.
- **Motion editor and animation preview**: docked under the viewport with a curve editor and keyframes view for every property — multi-select, box select, snapping, axis lock, easing picker, scrubbable playhead synced to the pixel-snapped (`50%`–`800%`) animation card, copy/paste, undo/redo, and zoom/pan.
- **Live gameplay preview**: a custom renderer ported from osu!lazer's source, running Argon Pro or any skin (including skins stored inside osu!lazer, with 2014 osu! Legacy fallbacks read from your local osu!lazer install when elements are unskinned). Plays random maps from your osu!lazer library, stable's `Songs` folder, or any `.osu`/`.osz`/`.olz`, with live Difficulty / Mods controls (CS, AR, HR, Hidden).
- **Skin editor**: open a skin folder, `.osk`, osu!lazer skin, or standalone PNG. Apply a non-destructive 32-bit float filter stack with per-filter toggles, undo/redo, carry-over edits, filter presets, batch apply, and full-skin `@2x`/`1x` scaling.
  - **Geometry & color**: ringing-free resize (smooth, crisp, or pixels; shrinking always area-averages), flip, rotate, crop, trim, expand border, color adjustments, tint, opacity, invert, blur, and halo-free sharpen.
  - **Restoration filters**: multi-mode deband (Pairs, Smooth, Gradient-aware) with grain, limit, narrowing, luma scaling, and edge protection; `dehalo_alpha` and `fine_dehalo`; NL-means and bilateral denoise; and edge-directed anti-aliasing.
  - **Inspection & saving**: before/after compare, checkerboard backdrop, `Boost` low-alpha/banding amplifier, 8-bit dithered or 16-bit PNG output, automatic original backups with restore, and `.osk` packing.
- **Skin artifact scanner**: scans every picture in a skin for banding, haloing, flat-region noise, and downscale ringing/fringe, showing thumbnails and jumping straight to the affected asset.
- **Element creator**: parametric 32-bit float SDF creator for standard `@2x` osu! gameplay pieces (hit circles, overlays, approach circles, cursors, slider pieces, reverse arrows, and more).
- **`skin.ini` editor**: edit metadata, combo colors, font prefixes, and behavior flags while preserving existing formatting and untouched sections (such as `[Mania]`).

## Planned

The skin editor is still incomplete and being built out alongside the followpoint animator. Planned next:

- Expanding the element creator to cover remaining skin pieces (judgement/hit bursts, spinner layers, score/combo/default digits, mod icons, and animated element sequences)
- Live gameplay preview of unsaved element creator and `skin.ini` edits
- Hit sound management (previewing, replacing, normalizing, and trimming samples)
- Skin completeness browser (showing skinned vs fallback elements across a skin at a glance)
- More testing on real Linux desktops (the Linux build has mostly been tested under WSL so far)

## Download

Windows and Linux builds are on the [releases page](../../releases).

## Checking the renderer against osu!lazer

The preview is Skin Studio's own renderer, written from osu!lazer's source so that it looks and moves like the game. To check it,
`dev/lazer-reference` is a small program built on the osu!lazer packages that plays a map with lazer's own renderer, and
`dev/compare.py` renders the same map, skin and times with both and puts the frames side by side (Windows only, development only;
it is not part of the release). `dotnet publish dev/lazer-reference -c Release` builds it.

## Build

    cargo build --release -p studio

The executable ends up in `target/release/`. For day-to-day use there is one more folder: `dist/` (not in git) holds the latest build as `skin-studio.exe` with its icon. `dev/host/` (not in git) is the built osu!lazer reference host that `dev/compare.py` uses.

On Linux you also need development packages for ALSA, fontconfig, xkbcommon, wayland/X11 and Vulkan or GL. On Debian/Ubuntu:

    sudo apt install build-essential pkg-config libasound2-dev libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev \
      libwayland-dev libx11-dev libxcb1-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libvulkan-dev

## License

MIT, see `LICENSE`. Parts are ported from osu!lazer (also MIT), see `NOTICE.md`. Not affiliated with or endorsed by ppy.
