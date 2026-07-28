# Enhancement: Render non-Latin track metadata correctly

**Status:** Completed  
**Issue:** [#20](https://github.com/sansoo1972/songart/issues/20)  
**Completed:** 2026-07-28

## Summary

SongArt now detects when the active themed fonts cannot render the current
metadata and switches the complete description panel to a coordinated Unicode
fallback family.

## Delivered implementation

- Bundled open-source Noto CJK sans, serif, and mono fonts with their SIL Open
  Font License.
- Checks every displayed metadata value for complete glyph coverage.
- Keeps the active themed fonts when all required glyphs are available.
- Switches the complete metadata panel, including labels and Latin-only values,
  to one fallback family when any field needs Unicode coverage.
- Matches the fallback style to the selected theme:
  - mono for `modern` and `techy`
  - serif for `retro`, `grungy`, and `fantasy`
  - sans for all remaining themes
- Normalizes metadata to composed Unicode before rendering so decomposed Korean
  Jamo display as complete Hangul syllables.
- Logs the fallback family and affected fields when fallback activates.
- Provides configurable fallback paths through `fonts.unicode_sans`,
  `fonts.unicode_serif`, and `fonts.unicode_mono`.

## Acceptance criteria

- [x] Korean and other supported non-Latin metadata display correctly.
- [x] Existing themed fonts remain active for metadata they support.
- [x] Fallback typography is visually consistent across the complete description
  panel.
- [x] Decomposed Korean metadata renders as composed Hangul syllables.
- [x] Fallback behavior and font paths are configurable and documented.
- [x] The completed behavior was verified on Raspberry Pi hardware.

## Validation

- `cargo check --locked --tests`
- `cargo check --release --locked`
- Runtime verification with Korean title and album metadata on Raspberry Pi
