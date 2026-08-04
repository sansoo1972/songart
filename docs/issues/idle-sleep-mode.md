# Enhancement: Add configurable idle sleep and artwork screensaver mode

**Status:** In progress  
**Issue:** [#45](https://github.com/sansoo1972/songart/issues/45)

## Summary

Add an application-level idle display that activates after 1–30 minutes without
a successfully recognized song. The app continues listening and wakes as soon
as SongRec identifies music again.

## Implementation

- Persistent enabled, timeout, and mode controls in the F1 settings overlay
- Smooth fade-to-black mode with no persistent UI elements
- Recent-artwork mode backed by a bounded in-memory session history
- One retained artwork entry per album, with a configurable 1–50 album limit
- Configurable artwork-mode blackout after 1–120 minutes (30 minutes by default)
- Mouse pointer hidden after inactivity and restored on mouse movement
- Black fallback when the current session has no artwork
- Recognition timestamp updated only for successful SongRec matches, including
  repeat detections of a song that is still playing
- Recognition failures and background processing do not wake the display

## Acceptance criteria

- [x] The inactivity timeout is configurable and cannot exceed 30 minutes.
- [x] Users can disable idle mode.
- [x] Users can choose recent-artwork or fade-to-black mode.
- [x] Successful song recognition resets the inactivity timer.
- [x] The selected idle mode begins after the configured period.
- [x] Artwork mode has a black fallback when no history is available.
- [x] Artwork mode keeps only one image for each album.
- [x] Artwork mode becomes fully black after the configured maximum idle period.
- [x] The mouse pointer is hidden when it is not in use.
- [x] Black mode fades smoothly and removes the visible UI.
- [x] A newly recognized song restores the normal now-playing display.
- [x] Recognition and audio capture continue while the display is idle.
- [x] The enabled state, timeout, and mode persist across restarts.
