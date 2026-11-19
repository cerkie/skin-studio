# Third-party material

- **Inter** (interface font): SIL Open Font License 1.1, see `apps/studio/ui/fonts/Inter-LICENSE.txt`. A copy of Inter SemiBold is
  also used by the Argon spinner labels (`crates/game/src/Inter-SemiBold.otf`).
- **Baloo 2** (only the digits 0-9 at weight 800, subset into `crates/game/src/Digits-Baloo2.ttf`, drawn on Argon hit circles): SIL Open Font
  License 1.1, copyright 2019 The Baloo 2 Project Authors (https://github.com/EkType/Baloo2); license text in `crates/game/Baloo2-OFL.txt`. Chosen as the
  closest free match to lazer's Torus digits.
- No textures or other assets from osu!lazer or osu-resources are included: the Argon pieces, including the approach ring and the repeat-arrow
  edge piece, are drawn in code to match lazer's look.
- **osu!lazer** (https://github.com/ppy/osu, MIT, copyright ppy Pty Ltd): parts of this project are ports of lazer's logic into Rust,
  which makes them derived from it: the Argon skin's shapes and animation timings, the cursor trail, slider and follow-point
  behavior, hit sounds and the interface's colors and sizes. The full license text is in `LICENSE-osu-MIT.txt` and has to stay
  with copies of this project. This project is not affiliated with or endorsed by ppy.
- Torus, lazer's own typeface, is a commercial font and is not included.
- `dev/lazer-reference` (not part of the release) is built on the osu!lazer packages (`ppy.osu.Game`, MIT) and is used to check Skin Studio's renderer against the game's.
