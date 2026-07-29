# Enhancement: Capture and display richer track metadata

**Status:** In progress — ready for Raspberry Pi validation  
**Issue:** [#44](https://github.com/sansoo1972/songart/issues/44)

## Summary

SongArt now captures optional track, release, identifier, and credit attributes
as structured state and presents meaningful configured values in a compact
secondary metadata panel.

## Implemented scope

- Structured album artist, track/disc number and totals, duration, lyricist,
  producer, ISRC, rating, copyright, catalog number, and record-label fields.
- SongRec/Shazam metadata remains the primary source.
- MusicBrainz ISRC search fills missing duration and release-specific values.
- MusicBrainz release details are accepted only when the album title matches.
- Matching MusicBrainz releases can supply track/disc position, album artist,
  label, catalog number, and full release date.
- Label values that merely duplicate the recognized artist are rejected.
- Artwork source URLs are logged only at debug level.
- Unknown optional values are omitted from the display.
- Two optional attributes are placed on each compact row.
- Field order and the maximum row count are configurable.
- Long values retain scrolling and clipping.
- Panel-wide Unicode fallback includes every optional value.

## Default display order

1. genre
2. composer
3. track
4. duration
5. label
6. ISRC
7. album artist
8. producer

The default four-row limit prevents the description panel from overlapping the
visualizer. Alternate supported fields can be selected in
`metadata_display.fields`.

## Validation

- `cargo check --locked --tests`
- `cargo check --release --locked`
- Exact MusicBrainz lookup verified for `I Am...I Said` by Neil Diamond:
  - album: `All-Time Greatest Hits`
  - track: `23/23`
  - duration: `3:34`
  - album artist: `Neil Diamond`
  - label: `Capitol Records`
  - catalog number: `B0020812-02`

## Remaining acceptance step

- Verify the compact metadata layout and live fallback results on Raspberry Pi
  in portrait and/or landscape mode.
