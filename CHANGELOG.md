# Changelog

All notable changes to `songart` will be documented in this file.

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
- [#4](https://github.com/sansoo1972/songart/issues/4) Improve portrait layout and vertical real estate usage
    - Portrait layout real-estate usage still needs refinement
    - Metadata text can become crowded with larger visualizer heights
- [#2](https://github.com/sansoo1972/songart/issues/2) Artwork reload pipeline still performs unnecessary reloads when artwork is unchanged
- [#3](https://github.com/sansoo1972/songart/issues/3) Logging timestamps are currently epoch-based and not human-friendly

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
