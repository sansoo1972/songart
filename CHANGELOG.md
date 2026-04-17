# Changelog

All notable changes to `songart` will be documented in this file.

## [Unreleased]

### Added
- External TOML-based runtime configuration
- `config/songart.toml` for operational settings
- `src/config.rs` for configuration structs and loader
- SDL-based fullscreen split-screen display
- Track metadata panel under artwork
- Timestamped logging with log levels
- Graceful Ctrl+C shutdown handling
- Structured now-playing debug output
- Placeholder UI state before first track recognition

### Changed
- Replaced framebuffer `fbi`-based display approach with SDL rendering
- Refactored application to use shared runtime context
- Moved environment-specific paths and settings out of `main.rs`
- Improved artwork candidate selection to prioritize higher-resolution Apple-hosted variants
- Improved duplicate suppression for repeated track/artwork states
- Switched to temp-file + rename for safer artwork updates

### Fixed
- Empty text rendering causing SDL_ttf “Text has zero width” errors
- Runtime artifact tracking issues by ignoring generated files
- Improved logging noise control through log levels instead of a simple verbose toggle

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
