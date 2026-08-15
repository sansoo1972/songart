# Enhancement: Add configurable idle sleep and artwork screensaver mode

**Status:** Completed

**Issue:** [#45](https://github.com/sansoo1972/songart/issues/45)

**Release:** 0.19.0

**Idle-pause fix release:** 0.19.1

**Completed:** 2026-08-04

**Fix verified:** 2026-08-15

## Summary

Add an application-level idle display that activates after 1–30 minutes without
a successfully recognized song. Sleep pauses audio capture and SongRec requests;
mouse activity or a non-Escape key wakes the app and resumes with fresh audio.

## Implementation

- Persistent enabled, timeout, and mode controls in the F1 settings overlay
- Smooth fade-to-black mode with no persistent UI elements
- Recent-artwork mode backed by a bounded in-memory session history
- One retained artwork entry per album, with a configurable 1–50 album limit
- Configurable artwork-mode blackout after 1–120 minutes (30 minutes by default)
- Floating artwork that bounces when it reaches a display edge
- Configurable post-artwork action: stay black or exit cleanly to the OS
- Mouse and non-Escape keyboard activity wake the display and reset its inactivity timer
- Escape exits SongArt from the active display or idle screens
- Mouse pointer hidden after inactivity and restored on mouse movement
- Black fallback when the current session has no artwork
- Recognition timestamp updated only for successful SongRec matches, including
  repeat detections of a song that is still playing
- Recognition failures and background processing do not wake the display
- Audio capture and SongRec submissions pause for the entire idle sleep period
- Pre-sleep buffered audio is discarded before capture and recognition resume

## Acceptance criteria

- [x] The inactivity timeout is configurable and cannot exceed 30 minutes.
- [x] Users can disable idle mode.
- [x] Users can choose recent-artwork or fade-to-black mode.
- [x] Successful song recognition resets the inactivity timer.
- [x] The selected idle mode begins after the configured period.
- [x] Artwork mode has a black fallback when no history is available.
- [x] Artwork mode keeps only one image for each album.
- [x] Artwork mode becomes fully black after the configured maximum idle period.
- [x] Artwork moves around the screen and bounces at its edges.
- [x] The maximum artwork-idle action can be configured as black or application exit.
- [x] Mouse and non-Escape keyboard activity wake the display before the terminal action.
- [x] Escape exits SongArt from either the active display or an idle screen.
- [x] The mouse pointer is hidden when it is not in use.
- [x] Black mode fades smoothly and removes the visible UI.
- [x] Mouse or non-Escape keyboard input restores the normal now-playing display.
- [x] Recognition and audio capture remain paused while the display is idle.
- [x] Wake restarts capture with an empty buffer before SongRec submissions resume.
- [x] The enabled state, timeout, and mode persist across restarts.
