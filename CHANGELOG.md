# Changelog

All notable changes to `songart` will be documented in this file.

## [Unreleased]

### Added
- External TOML-based runtime configuration
- `config/songart.toml` for runtime behavior
- `src/config.rs` for configuration structs and loader
- SDL-based artwork and metadata renderer
- Display presets for portrait and landscape scene definitions
- Theme-based font selection
- Theme-based font sizing
- Support for custom font assets under `assets/fonts`
- Timestamped logging with log levels
- Graceful Ctrl+C shutdown handling
- Structured now-playing debug output
- Placeholder UI state before first track recognition
- MIT license

### Changed
- Replaced framebuffer `fbi`-based display approach with SDL rendering
- Refactored the app around a shared runtime context
- Moved environment-specific settings out of `main.rs`
- Moved display sizing and layout spacing into TOML display presets
- Moved font sizing into TOML font theme definitions
- Changed display rendering so the configured scene is scaled to fit the actual SDL canvas
- Improved artwork candidate selection to prioritize higher-resolution Apple-hosted variants
- Improved duplicate suppression for repeated track/artwork states
- Switched to temp-file + rename for safer artwork updates
- Updated README to reflect config-driven operation and display/font presets

### Fixed
- Empty text rendering causing SDL_ttf “Text has zero width” errors
- Runtime artifact tracking issues by ignoring generated files
- Excessive render-loop canvas size log spam
- Logging noise control through log levels instead of a simple verbose toggle

---

## [0.1.0] - Initial working prototype

### Added
- Song recognition using SongRec
- JSON parsing of recognized track metadata
- Artwork download pipeline
- Fullscreen display of artwork using framebuffer tools
- Basic Rust runtime loop for recognition and display

### Notes
- Initial implementation focused on proving end-to-end functionality
- Later versions replaced framebuffer-only display with SDL-based layout rendering
