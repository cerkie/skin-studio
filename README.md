# Skin Studio

An editor for osu! followpoint animations with a live gameplay preview. Pick a skin and a map, edit on the left, and the changes show up in the preview right away.

- Effect presets, drawn sources or your own PNG, timing, keyframe motion and color, outline, shadow and debanding
- A motion editor docked under the view: a curve editor and a dope sheet for every property, with multi-select and box select, snapping, axis lock, a picker of easing curves, a scrubbable playhead tied to the animation card, copy and paste of keys, undo and redo, and zoom and pan
- A gameplay preview drawn the way osu!lazer draws it (the logic is ported from its source), with Argon Pro or any skin, including the skins stored in osu!lazer itself, on a random map from your osu!lazer library (or stable's Songs folder, or a `.osu`/`.osz`/`.olz` you choose); when a map ends the next random one starts
- The animation plays in a card over the preview that you can move, resize and fold away
- Export as `followpoint-N.png` (and `@2x`), import an existing set, and keep a history of your saves

## Planned

Followpoints are only the start. The goal is a full osu! skin editor, so these are planned, not done:

- Editing the other gameplay pieces the same way: hit circles and overlays, approach circles, slider ball, follow circle, ticks, reverse arrows, cursor and cursor trail, spinner, and the hit/judgement images
- Number and combo-number sprites, combo colors and `skin.ini` settings
- Exporting a complete `.osk`, and opening an existing skin to edit it instead of starting from scratch
- Hit sound management (previewing, replacing and trimming samples)
- Undo and redo
- More testing on real Linux desktops (the Linux build is only tested under WSL so far)

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
