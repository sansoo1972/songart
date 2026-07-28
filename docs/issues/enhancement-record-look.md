# Enhancement: Improve the look of the 33 1/3 record used during turn mode

**Status:** Completed  
**Issue:** [#41](https://github.com/sansoo1972/songart/issues/41)  
**Release:** 0.18.0  
**Completed:** 2026-07-28

## Summary
Improve the visual appearance of the 33 1/3 record graphic when the app enters its turn/rotation state so it looks more polished, more realistic, and more consistent with the overall UI.

## Problem
The current record representation looks too plain or generic during playback when the record is spinning. The visual treatment does not feel as premium or intentional as the rest of the interface, and the record could better communicate the nostalgic vinyl theme.

## Delivered implementation
The spinning 33 1/3 record now uses:

- a 2048×2048 neutral, glossy black vinyl material with dense pressed grooves
- a separate fixed lighting texture so reflections remain stationary while the record rotates
- continuous, independent album-label rotation
- a rounded rim, runout detail, and subtle pressing variation that makes rotation visible without rotating the light source
- Pi 3, Pi 4, and Pi 5 performance profiles at 10, 20, and 60 surface updates per second
- continuous display-frame-rate rotation so lower surface update profiles remain smooth
- high-resolution rendering suitable for 1080p and 4K displays

## Desired outcome
The 33 1/3 record should feel like a deliberate, high-quality visual element rather than a simple placeholder. It should look more polished while still remaining lightweight and readable during live playback.

## Acceptance criteria
- [x] The record visual is visibly improved in appearance when it is used in turn mode.
- [x] The updated design remains readable and visually balanced in the app UI.
- [x] The enhancement does not negatively impact performance or responsiveness.
- [x] The light source remains fixed while the vinyl material and album label rotate.
- [x] Pi 3, Pi 4, and Pi 5 users can select an appropriate animation profile from the interface.
- [x] The result is sharp on 1080p and 4K displays and visually matches the supplied reference more closely.

## Notes
This enhancement is focused on the visual quality and rendering performance of the record itself; it does not change music recognition or playback behavior.
