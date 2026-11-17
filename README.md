# Skin Studio

An editor for osu! followpoint animations with a live gameplay preview. Pick a skin and a map, edit on the left, and the changes show up in the preview right away.

- Effect presets, drawn sources or your own PNG, timing, keyframe motion and color, outline, shadow and debanding
- Preview with Argon Pro or any skin, on a random map from your osu!lazer library (or stable's Songs folder, or a `.osu`/`.osz`/`.olz` you choose)
- Export as `followpoint-N.png` (and `@2x`), import an existing set, and keep a history of your saves

## Planned

Followpoints are only the start. The goal is a full osu! skin editor, so these are planned, not done:

- Editing the other gameplay pieces the same way: hit circles and overlays, approach circles, slider ball, follow circle, ticks, reverse arrows, cursor and cursor trail, spinner, and the hit/judgement images
- Number and combo-number sprites, combo colors and `skin.ini` settings
- Exporting a complete `.osk`, and opening an existing skin to edit it instead of starting from scratch
- Hit sound management (previewing, replacing and trimming samples)
- Undo and redo
- macOS support, and more testing on real Linux desktops (the Linux build is only tested under WSL so far)

## Download

Windows and Linux builds are on the [releases page](../../releases).

## Build

    cargo build --release -p studio

On Linux you also need development packages for ALSA, fontconfig, xkbcommon, wayland/X11 and Vulkan or GL. On Debian/Ubuntu:

    sudo apt install build-essential pkg-config libasound2-dev libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev \
      libwayland-dev libx11-dev libxcb1-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libvulkan-dev

## License

MIT, see `LICENSE`. Parts are ported from osu!lazer (also MIT), see `NOTICE.md`. Not affiliated with or endorsed by ppy.
