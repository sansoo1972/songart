# Changelog

All notable changes to `songart` will be documented in this file.

---

## [Unreleased]

### Added
- [#39](https://github.com/sansoo1972/songart/issues/39) Optional segmented LED-style Spectrum analyzer rendering with lit active levels, configurable row thickness, configurable row/column gaps, and optional dim inactive rows.
- F1 settings overlay selection for Spectrum render style: `full`, `top_only`, and `segmented`.

---

## [0.16.0] - 2026-07-18

### Added
- [#35](https://github.com/sansoo1972/songart/issues/35) Application-level SDL output rotation with `display.rotation` values `normal`, `clockwise`, `inverted`, and `counter_clockwise`.
- F1 settings overlay controls for saving display orientation and output rotation.
- [#34](https://github.com/sansoo1972/songart/issues/34) Font-theme diagnostics log metadata inputs, selected theme, and loaded theme on text cache rebuilds.

### Changed
- SongArt now renders the logical scene to an off-screen texture and applies final output rotation once when presenting the composed frame.
- Windowed startup dimensions are swapped for 90-degree and 270-degree output rotations while preserving logical layout presets.
- Landscape layout now places album artwork on the right with metadata on the left and the visualizer beneath the metadata column.
- Metadata-driven font selection now prefers explicit genre matches before broad release-year fallback rules, making theme changes more predictable across tracks.
- Bundled `modern` and `techy` font presets now use more distinct font pairings so metadata theme changes are visibly obvious.
- Invalid `fonts.mode` values now log a warning and fall back to metadata selection instead of silently using the fixed theme.

---

## [0.15.0] - 2026-07-05

### Added
- [#8](https://github.com/sansoo1972/songart/issues/8) Keyboard-accessible settings overlay for changing display options while music continues running.
- Live artwork selection between standard cover and turntable presentation.
- Live visualizer selection between Spectrum, Oscilloscope, and Analog VU.
- Shared sensitivity slider that adjusts visualizer gain from `0.25` to `8.0`.
- Safe configuration saving that preserves TOML comments, writes through a temporary file, and creates `config/songart.toml.bak`.

### Changed
- Settings use a dedicated sans-serif font and no longer inherit the active song-information theme.
- Artwork mode, visualizer mode, and sensitivity can be previewed before saving.

---

## [0.14.0] - 2026-07-05

### Added
- [#31](https://github.com/sansoo1972/songart/issues/31) Dedicated dual analog VU renderer inspired by illuminated 1970s hi-fi equipment.
- Photorealistic aged meter faces with classic sans-serif dB markings, a red overload band, recessed housings, brass trim, and glass detail.
- Animated mechanical needles with logarithmic level mapping, shadows, metal pivot hubs, fast rise, and damped return.
- Dynamic jewel-style peak indicators that illuminate when the smoothed signal enters overload.
- README preview of the vintage VU presentation.

### Changed
- `analog_vu` now uses its own renderer instead of being routed through the oscilloscope drawing path.
- Static meter artwork is texture-backed so only needles and peak illumination require per-frame drawing.
- Analog VU mode can run alongside the existing turntable artwork mode.

---

## [0.13.0] - 2026-06-26

### Added
- [#13](https://github.com/sansoo1972/songart/issues/13) Optional `turntable` artwork mode that presents album art as the center label of a 33⅓ RPM vinyl record.
- Five-second full-cover presentation followed by a soft circular crop and animated shrink into the record label.
- Realistic vinyl treatment with a dense continuous spiral groove, five track regions, runout detail, outer rim, and center spindle.
- Crossfade from the outgoing spinning record into newly identified full-size artwork.

### Changed
- Artwork rendering is now performed dynamically so track transitions and turntable animation remain smooth while metadata and visualizers continue rendering.
- The existing static `cover` artwork mode remains the default and fallback presentation.

---

## [0.12.1] - 2026-06-23

### Added
- [#22](https://github.com/sansoo1972/songart/issues/22) Oscilloscope mode now renders as a polished scope view with a graticule grid, clipped trace area, layered trace glow, and higher-density default sampling.

---

## [0.12.0] - 2026-06-22

### Added
- Long song title, artist, album, year, genre, and composer values now scroll horizontally when they exceed the available display width while labels remain fixed.
- Overflowing metadata values now loop continuously instead of snapping back after reaching the end.

### Fixed
- [#19](https://github.com/sansoo1972/songart/issues/19) Composer metadata now falls back to a MusicBrainz ISRC lookup when SongRec/Shazam metadata does not include composer or writer fields.

---

## [0.11.0] - 2026-06-22

### Added
- Spectrum analyzer `top_only` rendering mode that uses the full analyzer height for a top-edge spectrum display.
- Configurable `top_only_height_ratio` for drawing only the active upper portion of each spectrum bar.
- Optional per-bar spectrum peak hold/drop-off markers.
- Configurable peak marker behavior:
  - `enabled`
  - `hold_ms`
  - `drop_pixels`
  - `color`
  - `use_bar_color`

### Changed
- Spectrum rendering now supports both the existing mirrored `full` mode and the new full-height `top_only` mode.
- Peak marker rendering works with artwork-derived palette colors or a fixed configured color.

### Fixed
- [#15](https://github.com/sansoo1972/songart/issues/15) Spectrum bars can now render as a top-only display instead of full mirrored bars.
- [#16](https://github.com/sansoo1972/songart/issues/16) Spectrum bars now support peak hold/drop-off markers.

---

## [0.10.0] - 2026-06-15

### Added
- Artwork-derived visualizer colors for spectrum and oscilloscope rendering.
- Configurable visualizer color mode:
  - `fixed` for explicit configured colors
  - `artwork` for colors extracted from the current album artwork
- Configurable fallback visualizer colors when artwork loading or color extraction fails.
- Artwork palette extraction controls:
  - minimum perceived brightness
  - minimum saturation
  - palette size
  - hue bucket count

### Changed
- Spectrum analyzer bars now sweep through a broader artwork-derived color palette instead of using only two fixed colors.
- Lower spectrum bars use the palette in reverse order for stronger visual variation.
- Visualizer color selection remains config-driven and preserves fixed/fallback behavior.

### Fixed
- [#12](https://github.com/sansoo1972/songart/issues/12) Visualizer colors can now be derived from album artwork while falling back safely to configured colors.

### Notes
- Artwork palette extraction intentionally favors bright, saturated colors to avoid muddy gray/brown output.

---

## [0.9.2] - 2026-05-08

### Added
- Human-readable local log timestamps for easier troubleshooting.
- Metadata-driven font theme selection using song genre and release year.
- Font mode configuration:
  - `fixed` for a manually selected theme
  - `metadata` for automatic theme selection based on track metadata
- Fallback font theme support when metadata does not match a known rule.
- Configurable display region background colors:
  - overall canvas background
  - artwork/top-region background
  - metadata-region background
  - visualizer/analyzer background
- Cleaner `songart.toml` organization with grouped sections and clearer comments.
- Cleaner `config.rs` organization with dedicated display color configuration.
- Cleaner `display.rs` organization with separated helpers for colors, metadata text, font selection, visualizer drawing, static scene rendering, and runtime display loop behavior.

### Changed
- Improved portrait layout defaults for native 1080x1920 displays.
- Increased visualizer height in portrait mode for stronger visual presence.
- Increased spectrum analyzer bin count for more detail.
- Tuned spectrum analyzer responsiveness:
  - faster attack
  - smoother falloff
  - improved contrast
  - adjusted noise floor
  - updated log scaling values
- Updated portrait layout spacing to better use vertical real estate.
- Updated landscape preset to use standard 1920x1080 dimensions.
- Updated default configuration to use metadata-driven typography.
- Updated rendering so artwork, metadata, and visualizer regions can share a seamless flat black background while remaining independently configurable.

### Fixed
- [#3](https://github.com/sansoo1972/songart/issues/3) Logging timestamps are now human-readable instead of epoch-based.
- [#4](https://github.com/sansoo1972/songart/issues/4) Portrait layout now makes better use of vertical real estate.
- [#7](https://github.com/sansoo1972/songart/issues/7) Font selection can now be metadata-driven instead of only manually selected.
- [#9](https://github.com/sansoo1972/songart/issues/9) Spectrum analyzer size and responsiveness have been improved.
- Fullscreen pixelation caused by a 720x1280 portrait preset on a 1080x1920 display was resolved by updating the portrait preset to native resolution.

### Notes
- Current metadata-driven font choices are functional and intentionally configurable; individual font-theme mappings may be refined stylistically in later releases.
- Display background colors are now configurable per major region, enabling both seamless all-black layouts and future themed region styling.

---

## [0.9.1] - 2026-05-06

### Added
- Real-time FFT spectrum visualizer
- Log-spaced spectrum band processing
- Hann-windowed FFT analysis pipeline
- Shared rolling audio buffer for recognition and visualization
- Spectrum smoothing controls
- Configurable FFT sizing and frequency range
- Renderer scene caching for improved display efficiency
- Visualizer diagnostic logging:
  - max spectrum bin
  - average spectrum energy
  - live RMS level
- Scaffolded `renderer/` module structure for future rendering separation

### Changed
- Replaced the original digital VU meter with a spectrum analyzer mode
- Moved visualizer processing to true real-time shared audio analysis
- Reduced spectrum saturation by switching away from auto-peak normalization
- Tuned spectrum density and spacing for portrait displays
- Improved metadata refresh behavior when tracks change on the same album artwork
- Improved renderer update flow using scene versioning
- Reduced excessive renderer rebuilds
- Expanded visualizer configuration in `songart.toml`

### Fixed
- Same-artwork track changes not updating metadata
- Excessive UI refresh skips during album-based playback
- [#1](https://github.com/sansoo1972/songart/issues/1) Replace polling visualizer with real-time FFT audio pipeline
    - Visualizer responsiveness and sluggishness
    - Spectrum over-amplification caused by aggressive normalization

### Known Issues
- [#2](https://github.com/sansoo1972/songart/issues/2) Artwork reload pipeline still performs unnecessary reloads when artwork is unchanged

---

## [0.9.0] - 2026-04-17

### Added
- Modular source layout:
  - `main.rs`
  - `config.rs`
  - `logging.rs`
  - `state.rs`
  - `audio.rs`
  - `recognition.rs`
  - `display.rs`
- External TOML-based runtime configuration
- Config-driven display presets for portrait and landscape
- Theme-based font selection
- Theme-based font sizing
- Support for custom font assets under `assets/fonts`
- Live digital VU meter
- Visualizer configuration block in `songart.toml`
- Timestamped logging with log levels
- Structured now-playing debug output
- Graceful Ctrl+C shutdown handling
- MIT license

### Changed
- Replaced framebuffer `fbi`-based display approach with SDL rendering
- Refactored the application around shared app/song/meter state
- Split a large single-file runtime into focused modules
- Moved environment-specific settings out of `main.rs`
- Moved display sizing and layout spacing into TOML display presets
- Moved font sizing into TOML font theme definitions
- Changed display rendering so the configured scene is scaled to fit the actual SDL canvas
- Improved artwork candidate selection to prioritize higher-resolution Apple-hosted variants
- Improved duplicate suppression for repeated track/artwork states
- Switched to temp-file + rename for safer artwork updates
- Updated project docs to reflect config-driven operation and modular architecture

### Fixed
- Empty text rendering causing SDL_ttf “Text has zero width” errors
- Runtime artifact tracking issues by ignoring generated files
- Excessive render-loop canvas size log spam
- Logging noise control through log levels instead of a simple verbose toggle
