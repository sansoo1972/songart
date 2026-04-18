# Changelog

All notable changes to `songart` will be documented in this file.

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

---

## [1.0.0]

### Added
- Song recognition using SongRec
- JSON parsing of recognized track metadata
- Artwork download pipeline
- Fullscreen display of artwork using framebuffer tools
- Basic Rust runtime loop for recognition and display

### Notes
- Initial implementation focused on proving end-to-end functionality
- Later versions replaced framebuffer-only display with SDL-based layout rendering
