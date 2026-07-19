use crate::audio::{SharedAudioBuffer, build_oscilloscope_points, compute_rms};
use crate::config::DisplayPreset;
use crate::fft::compute_spectrum_bins;
use crate::logging::{log_debug, log_error, log_info};
use crate::state::{AppContext, SongState};
use crate::visualizer::VisualizerMode;

use sdl2::event::Event;
use sdl2::image::{InitFlag, LoadSurface, LoadTexture};
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::{Point, Rect};
use sdl2::render::{BlendMode, Texture, TextureCreator, TextureQuery};
use sdl2::surface::Surface;
use sdl2::video::WindowContext;

use std::fs;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

// ==============================================================================
// Text + Metadata Helpers
// ==============================================================================

fn album_line(state: &SongState) -> String {
    if state.album.trim().is_empty() || state.album == "Unknown" {
        "Album unknown".to_string()
    } else {
        state.album.clone()
    }
}

fn release_year_line(state: &SongState) -> String {
    if let Some(year) = parse_release_year(&state.released) {
        year.to_string()
    } else if state.released.trim().is_empty() || state.released == "Unknown" {
        "Unknown".to_string()
    } else {
        state.released.clone()
    }
}

// ==============================================================================
// Color Helpers
// ==============================================================================

fn parse_hex_color(value: &str, fallback: Color) -> Color {
    let hex = value.trim().trim_start_matches('#');

    if hex.len() != 6 {
        return fallback;
    }

    let r = u8::from_str_radix(&hex[0..2], 16);
    let g = u8::from_str_radix(&hex[2..4], 16);
    let b = u8::from_str_radix(&hex[4..6], 16);

    match (r, g, b) {
        (Ok(r), Ok(g), Ok(b)) => Color::RGB(r, g, b),
        _ => fallback,
    }
}

fn dim_color(color: Color, factor: f32) -> Color {
    let factor = factor.clamp(0.0, 1.0);
    Color::RGB(
        ((color.r as f32) * factor) as u8,
        ((color.g as f32) * factor) as u8,
        ((color.b as f32) * factor) as u8,
    )
}

fn canvas_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(&ctx.config.display.colors.background, Color::RGB(0, 0, 0))
}

fn artwork_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.artwork_background,
        canvas_background_color(ctx),
    )
}

fn metadata_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.metadata_background,
        canvas_background_color(ctx),
    )
}

fn visualizer_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.visualizer_background,
        canvas_background_color(ctx),
    )
}

#[derive(Clone, Debug)]
struct VisualizerDrawColors {
    upper: Color,
    lower: Color,
    palette: Vec<Color>,
}

impl VisualizerDrawColors {
    fn fixed(ctx: &AppContext) -> Self {
        let upper = parse_hex_color(
            &ctx.config.visualizer.colors.upper,
            Color::RGB(80, 220, 120),
        );
        let lower = parse_hex_color(
            &ctx.config.visualizer.colors.lower,
            Color::RGB(80, 160, 255),
        );

        Self {
            upper,
            lower,
            palette: vec![upper, lower],
        }
    }

    fn fallback(ctx: &AppContext) -> Self {
        let upper = parse_hex_color(
            &ctx.config.visualizer.colors.fallback_upper,
            Color::RGB(80, 220, 120),
        );
        let lower = parse_hex_color(
            &ctx.config.visualizer.colors.fallback_lower,
            Color::RGB(80, 160, 255),
        );

        Self {
            upper,
            lower,
            palette: vec![upper, lower],
        }
    }
}

#[derive(Clone, Debug)]
struct RuntimeSpectrumSettings {
    render_style: String,
    segment_rows: u32,
    segment_height: u32,
    segment_gap: u32,
    segment_column_gap: u32,
    segment_inactive: bool,
}

impl RuntimeSpectrumSettings {
    fn from_config(ctx: &AppContext) -> Self {
        let spectrum = &ctx.config.visualizer.spectrum;
        Self {
            render_style: spectrum.render_style.clone(),
            segment_rows: spectrum.segment_rows,
            segment_height: spectrum.segment_height,
            segment_gap: spectrum.segment_gap,
            segment_column_gap: spectrum.segment_column_gap,
            segment_inactive: spectrum.segment_inactive,
        }
    }

    fn segmented(&self) -> bool {
        self.render_style.eq_ignore_ascii_case("segmented")
    }

    fn top_only(&self) -> bool {
        self.render_style.eq_ignore_ascii_case("top_only")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsRow {
    Artwork,
    Visualizer,
    SpectrumStyle,
    SegmentRows,
    SegmentHeight,
    SegmentGap,
    SegmentColumnGap,
    SegmentInactive,
    Sensitivity,
    Orientation,
    Rotation,
}

fn settings_rows(visualizer_mode: &str, spectrum: &RuntimeSpectrumSettings) -> Vec<SettingsRow> {
    let mut rows = vec![SettingsRow::Artwork, SettingsRow::Visualizer];

    if visualizer_mode.eq_ignore_ascii_case("spectrum") {
        rows.push(SettingsRow::SpectrumStyle);

        if spectrum.segmented() {
            rows.push(SettingsRow::SegmentRows);
            rows.push(SettingsRow::SegmentHeight);
            rows.push(SettingsRow::SegmentGap);
            rows.push(SettingsRow::SegmentColumnGap);
            rows.push(SettingsRow::SegmentInactive);
        }
    }

    rows.push(SettingsRow::Sensitivity);
    rows.push(SettingsRow::Orientation);
    rows.push(SettingsRow::Rotation);
    rows
}

#[derive(Clone, Copy)]
struct ArtworkColorCandidate {
    color: Color,
    hue: f32,
    score: f32,
}

#[derive(Clone, Copy, Default)]
struct HueBucket {
    red_sum: f32,
    green_sum: f32,
    blue_sum: f32,
    score_sum: f32,
    count: usize,
}

impl HueBucket {
    fn push(&mut self, r: u8, g: u8, b: u8, score: f32) {
        self.red_sum += (r as f32) * score;
        self.green_sum += (g as f32) * score;
        self.blue_sum += (b as f32) * score;
        self.score_sum += score;
        self.count += 1;
    }

    fn candidate(self, hue: f32) -> Option<ArtworkColorCandidate> {
        if self.count == 0 || self.score_sum <= 0.0 {
            return None;
        }

        Some(ArtworkColorCandidate {
            color: Color::RGB(
                (self.red_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
                (self.green_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
                (self.blue_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
            ),
            hue,
            score: self.score_sum,
        })
    }
}

fn color_channels(color: Color) -> (u8, u8, u8) {
    (color.r, color.g, color.b)
}

fn perceived_brightness(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * (r as f32)) + (0.587 * (g as f32)) + (0.114 * (b as f32))
}

fn rgb_saturation(r: u8, g: u8, b: u8) -> f32 {
    let max = r.max(g).max(b) as f32;
    let min = r.min(g).min(b) as f32;

    if max <= 0.0 { 0.0 } else { (max - min) / max }
}

fn rgb_hue(r: u8, g: u8, b: u8) -> f32 {
    let rf = (r as f32) / 255.0;
    let gf = (g as f32) / 255.0;
    let bf = (b as f32) / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    if delta <= f32::EPSILON {
        return 0.0;
    }

    let hue = if (max - rf).abs() <= f32::EPSILON {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if (max - gf).abs() <= f32::EPSILON {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    if hue < 0.0 { hue + 360.0 } else { hue }
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs();
    diff.min(360.0 - diff)
}

fn pick_spread_palette(
    candidates: &[ArtworkColorCandidate],
    palette_size: usize,
) -> Vec<ArtworkColorCandidate> {
    let target_size = palette_size.clamp(2, 12);
    let mut palette = Vec::new();

    for min_distance in [55.0, 40.0, 25.0, 0.0] {
        for candidate in candidates {
            if palette.len() >= target_size {
                return palette;
            }

            let is_distinct = palette.iter().all(|selected: &ArtworkColorCandidate| {
                hue_distance(selected.hue, candidate.hue) >= min_distance
            });

            if is_distinct {
                palette.push(*candidate);
            }
        }
    }

    palette
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) + ((b as f32) - (a as f32)) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn palette_color_at(palette: &[Color], index: usize, count: usize) -> Color {
    match palette {
        [] => Color::RGB(80, 220, 120),
        [color] => *color,
        colors => {
            let denom = count.saturating_sub(1).max(1) as f32;
            let position = (index as f32 / denom) * ((colors.len() - 1) as f32);
            let left = position.floor() as usize;
            let right = (left + 1).min(colors.len() - 1);
            let t = position - (left as f32);
            let a = colors[left];
            let b = colors[right];

            Color::RGB(
                lerp_channel(a.r, b.r, t),
                lerp_channel(a.g, b.g, t),
                lerp_channel(a.b, b.b, t),
            )
        }
    }
}

fn extract_visualizer_colors_from_artwork(
    ctx: &AppContext,
    artwork_path: &str,
) -> Result<VisualizerDrawColors, String> {
    let surface = Surface::from_file(artwork_path)?;
    let surface = surface.convert_format(PixelFormatEnum::RGB24)?;
    let pixels = surface
        .without_lock()
        .ok_or_else(|| "Artwork pixel buffer requires locking".to_string())?;

    let width = surface.width() as usize;
    let height = surface.height() as usize;
    let pitch = surface.pitch() as usize;
    let pixel_count = width.saturating_mul(height);

    if width == 0 || height == 0 || pixel_count == 0 {
        return Err("Artwork has no pixels".to_string());
    }

    let sample_stride = (pixel_count / 4096).max(1);
    let min_brightness = ctx.config.visualizer.colors.min_brightness as f32;
    let min_saturation = ctx.config.visualizer.colors.min_saturation.clamp(0.0, 1.0);
    let hue_bucket_count = ctx.config.visualizer.colors.hue_bucket_count.clamp(3, 36);
    let mut buckets = vec![HueBucket::default(); hue_bucket_count];

    for i in (0..pixel_count).step_by(sample_stride) {
        let x = i % width;
        let y = i / width;
        let offset = y.saturating_mul(pitch) + x.saturating_mul(3);

        if offset + 2 >= pixels.len() {
            continue;
        }

        let r = pixels[offset];
        let g = pixels[offset + 1];
        let b = pixels[offset + 2];
        let brightness = perceived_brightness(r, g, b);
        let saturation = rgb_saturation(r, g, b);

        if brightness < min_brightness || saturation < min_saturation {
            continue;
        }

        let hue = rgb_hue(r, g, b);
        let bucket_index =
            ((hue / 360.0) * (hue_bucket_count as f32)).floor() as usize % hue_bucket_count;
        let score = saturation * 2.0 + (brightness / 255.0);

        buckets[bucket_index].push(r, g, b, score);
    }

    let bucket_width = 360.0 / (hue_bucket_count as f32);
    let mut candidates: Vec<ArtworkColorCandidate> = buckets
        .into_iter()
        .enumerate()
        .filter_map(|(index, bucket)| bucket.candidate((index as f32 + 0.5) * bucket_width))
        .collect();

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let palette = pick_spread_palette(&candidates, ctx.config.visualizer.colors.palette_size);
    let upper = palette
        .first()
        .copied()
        .ok_or_else(|| "Artwork did not contain enough bright saturated pixels".to_string())?;

    let lower = palette
        .iter()
        .copied()
        .find(|candidate| hue_distance(candidate.hue, upper.hue) >= 35.0)
        .or_else(|| palette.get(1).copied())
        .unwrap_or(upper);

    Ok(VisualizerDrawColors {
        upper: upper.color,
        lower: lower.color,
        palette: palette
            .into_iter()
            .map(|candidate| candidate.color)
            .collect(),
    })
}

fn visualizer_colors_for_artwork(
    ctx: &AppContext,
    artwork_path: Option<&str>,
) -> VisualizerDrawColors {
    match ctx
        .config
        .visualizer
        .colors
        .mode
        .to_ascii_lowercase()
        .as_str()
    {
        "artwork" => {
            if let Some(path) = artwork_path {
                match extract_visualizer_colors_from_artwork(ctx, path) {
                    Ok(colors) => {
                        let (ur, ug, ub) = color_channels(colors.upper);
                        let (lr, lg, lb) = color_channels(colors.lower);
                        log_debug(
                            ctx,
                            &format!(
                                "Visualizer colors derived from artwork: upper=#{:02X}{:02X}{:02X} lower=#{:02X}{:02X}{:02X} palette_colors={}",
                                ur,
                                ug,
                                ub,
                                lr,
                                lg,
                                lb,
                                colors.palette.len()
                            ),
                        );
                        colors
                    }
                    Err(e) => {
                        log_error(
                            ctx,
                            &format!(
                                "Failed to derive visualizer colors from artwork; using fallback colors: {e}"
                            ),
                        );
                        VisualizerDrawColors::fallback(ctx)
                    }
                }
            } else {
                VisualizerDrawColors::fallback(ctx)
            }
        }
        "fixed" => VisualizerDrawColors::fixed(ctx),
        other => {
            log_error(
                ctx,
                &format!(
                    "Unknown visualizer color mode '{}'; using fallback colors",
                    other
                ),
            );
            VisualizerDrawColors::fallback(ctx)
        }
    }
}

// ==============================================================================
// Font Theme Selection
// ==============================================================================

fn parse_release_year(released: &str) -> Option<i32> {
    let digits: String = released
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(4)
        .collect();

    if digits.len() == 4 {
        digits.parse::<i32>().ok()
    } else {
        None
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn metadata_font_theme_name(genre: &str, released: &str, fallback_theme: &str) -> String {
    let genre = genre.to_ascii_lowercase();
    let year = parse_release_year(released);

    // Prefer explicit genre information over broad release-era assumptions.
    if contains_any(
        &genre,
        &["electronic", "synth", "synth-pop", "new wave", "dance"],
    ) {
        return "techy".to_string();
    }

    if contains_any(
        &genre,
        &["rock", "alternative", "grunge", "punk", "metal", "indie"],
    ) {
        return "grungy".to_string();
    }

    if contains_any(&genre, &["classical", "soundtrack", "score", "orchestral"]) {
        return "fantasy".to_string();
    }

    if contains_any(
        &genre,
        &[
            "folk",
            "acoustic",
            "country",
            "singer-songwriter",
            "latin",
            "spanish",
            "mexicano",
            "salsa",
            "bachata",
            "reggaeton",
        ],
    ) {
        return "scripted".to_string();
    }

    if contains_any(
        &genre,
        &["jazz", "blues", "soul", "funk", "disco", "oldies"],
    ) {
        return "retro".to_string();
    }

    if contains_any(&genre, &["pop", "r&b", "hip-hop", "rap", "urban"]) {
        return "modern".to_string();
    }

    // Use release era only when genre is absent or does not match a rule.
    match year {
        Some(..=1979) => return "retro".to_string(),
        Some(1980..=1989) => return "techy".to_string(),
        Some(1990..=1999) => return "grungy".to_string(),
        Some(2000..) => return "modern".to_string(),
        None => {}
    }

    fallback_theme.to_ascii_lowercase()
}

fn selected_font_theme_name(
    font_mode: &str,
    fixed_theme: &str,
    genre: &str,
    released: &str,
    fallback_theme: &str,
) -> (String, bool) {
    match font_mode.trim().to_ascii_lowercase().as_str() {
        "fixed" => (fixed_theme.trim().to_ascii_lowercase(), false),
        "metadata" => (
            metadata_font_theme_name(genre, released, fallback_theme),
            false,
        ),
        _ => (
            metadata_font_theme_name(genre, released, fallback_theme),
            true,
        ),
    }
}

fn selected_font_theme(ctx: &AppContext, state: &SongState) -> String {
    let (theme, invalid_mode) = selected_font_theme_name(
        &ctx.config.fonts.mode,
        &ctx.config.fonts.theme,
        &state.genre,
        &state.released,
        &ctx.config.fonts.fallback_theme,
    );

    if invalid_mode {
        log_error(
            ctx,
            &format!(
                "Invalid fonts.mode '{}'; using metadata font selection",
                ctx.config.fonts.mode
            ),
        );
    }

    theme
}

fn selected_fonts<'a>(
    ctx: &'a AppContext,
    state: &SongState,
) -> (&'a str, &'a str, u16, u16, String) {
    let mut theme_name = selected_font_theme(ctx, state);

    if !ctx.config.font_themes.contains_key(&theme_name) {
        log_error(
            ctx,
            &format!(
                "Configured font theme '{}' was not found; falling back to '{}'",
                theme_name, ctx.config.fonts.fallback_theme
            ),
        );

        theme_name = ctx.config.fonts.fallback_theme.to_ascii_lowercase();
    }

    if let Some(theme) = ctx.config.font_themes.get(&theme_name) {
        (
            &theme.title,
            &theme.body,
            theme.title_size,
            theme.body_size,
            theme_name,
        )
    } else {
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            34,
            24,
            "system-fallback".to_string(),
        )
    }
}

// ==============================================================================
// Display Preset Selection
// ==============================================================================

fn selected_display_preset<'a>(ctx: &'a AppContext) -> Option<&'a DisplayPreset> {
    let key = ctx.config.display.orientation.to_ascii_lowercase();
    ctx.config.display_presets.get(&key)
}

// ==============================================================================
// Scene Layout
// ==============================================================================

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SceneLayout {
    artwork_region: Rect,
    metadata_region: Rect,
    visualizer_region: Rect,
}

fn scene_layout(preset: &DisplayPreset) -> SceneLayout {
    if preset.width > preset.height {
        let outer_padding = 40u32;
        let gap = 36u32;
        let left_w = ((preset.width as f32) * 0.43) as u32;
        let right_x = outer_padding + left_w + gap;
        let right_w = preset
            .width
            .saturating_sub(right_x)
            .saturating_sub(outer_padding);
        let visualizer_h = 320u32.min(preset.height.saturating_sub(outer_padding * 2 + gap));
        let metadata_h = preset
            .height
            .saturating_sub(outer_padding * 2 + gap + visualizer_h);

        SceneLayout {
            metadata_region: Rect::new(
                outer_padding as i32,
                outer_padding as i32,
                left_w,
                metadata_h,
            ),
            visualizer_region: Rect::new(
                outer_padding as i32,
                (outer_padding + metadata_h + gap) as i32,
                left_w,
                visualizer_h,
            ),
            artwork_region: Rect::new(
                right_x as i32,
                outer_padding as i32,
                right_w,
                preset.height - outer_padding * 2,
            ),
        }
    } else {
        let top_h = ((preset.height as f32) * preset.top_panel_ratio) as u32;
        SceneLayout {
            artwork_region: Rect::new(0, 0, preset.width, top_h),
            metadata_region: Rect::new(0, top_h as i32, preset.width, preset.height - top_h),
            visualizer_region: Rect::new(0, top_h as i32, preset.width, preset.height - top_h),
        }
    }
}

// ==============================================================================
// Cached Text
// ==============================================================================

struct CachedText<'a> {
    texture: Texture<'a>,
    rect: Rect,
    viewport_width: u32,
}

impl<'a> CachedText<'a> {
    fn new(
        texture_creator: &'a TextureCreator<WindowContext>,
        font: &sdl2::ttf::Font,
        text: &str,
        color: Color,
        x: i32,
        y: i32,
        viewport_width: u32,
    ) -> Result<Self, String> {
        let safe_text = if text.trim().is_empty() { " " } else { text };

        let surface = font
            .render(safe_text)
            .blended(color)
            .map_err(|e| e.to_string())?;

        let texture = texture_creator
            .create_texture_from_surface(&surface)
            .map_err(|e| e.to_string())?;

        let rect = Rect::new(x, y, surface.width(), surface.height());

        Ok(Self {
            texture,
            rect,
            viewport_width,
        })
    }

    fn scroll_offset(&self, elapsed: Duration) -> i32 {
        const START_PAUSE_SECS: f32 = 5.0;
        const SCROLL_PIXELS_PER_SEC: f32 = 55.0;
        const LOOP_GAP_PIXELS: u32 = 28;

        if self.rect.width() <= self.viewport_width {
            return 0;
        }

        let loop_distance = self.rect.width() + LOOP_GAP_PIXELS;
        let scroll_secs = (loop_distance as f32) / SCROLL_PIXELS_PER_SEC;
        let cycle_secs = START_PAUSE_SECS + scroll_secs;
        let cycle_pos = elapsed.as_secs_f32() % cycle_secs;

        if cycle_pos < START_PAUSE_SECS {
            0
        } else {
            ((cycle_pos - START_PAUSE_SECS) * SCROLL_PIXELS_PER_SEC)
                .round()
                .clamp(0.0, loop_distance as f32) as i32
        }
    }

    fn draw(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        const LOOP_GAP_PIXELS: i32 = 28;

        let scroll_x = self.scroll_offset(elapsed);
        let base_x = offset_x + (((self.rect.x() - scroll_x) as f32 * scale) as i32);
        let base_y = offset_y + (((self.rect.y()) as f32 * scale) as i32);
        let texture_w = ((self.rect.width() as f32) * scale).max(1.0) as u32;
        let texture_h = ((self.rect.height() as f32) * scale).max(1.0) as u32;
        let viewport_x = offset_x + (((self.rect.x()) as f32 * scale) as i32);

        let clip = Rect::new(
            viewport_x,
            base_y,
            ((self.viewport_width as f32) * scale).max(1.0) as u32,
            texture_h,
        );

        canvas.set_clip_rect(Some(clip));
        let result = canvas.copy(
            &self.texture,
            None,
            Rect::new(base_x, base_y, texture_w, texture_h),
        );

        let second_result = if result.is_ok() && self.rect.width() > self.viewport_width {
            let second_x =
                base_x + (((self.rect.width() as i32 + LOOP_GAP_PIXELS) as f32 * scale) as i32);
            canvas.copy(
                &self.texture,
                None,
                Rect::new(second_x, base_y, texture_w, texture_h),
            )
        } else {
            Ok(())
        };

        canvas.set_clip_rect(None);

        result?;
        second_result?;
        Ok(())
    }
}

struct TextField<'a> {
    label: CachedText<'a>,
    value: CachedText<'a>,
}

impl<'a> TextField<'a> {
    fn new(
        texture_creator: &'a TextureCreator<WindowContext>,
        font: &sdl2::ttf::Font,
        label: &str,
        value: &str,
        label_color: Color,
        value_color: Color,
        x: i32,
        y: i32,
        viewport_width: u32,
    ) -> Result<Self, String> {
        let label = CachedText::new(
            texture_creator,
            font,
            label,
            label_color,
            x,
            y,
            viewport_width,
        )?;
        let value_x = x + label.rect.width() as i32;
        let value_viewport_width = viewport_width.saturating_sub(label.rect.width()).max(1);
        let value = CachedText::new(
            texture_creator,
            font,
            value,
            value_color,
            value_x,
            y,
            value_viewport_width,
        )?;

        Ok(Self { label, value })
    }

    fn draw(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        self.label
            .draw(canvas, offset_x, offset_y, scale, Duration::ZERO)?;
        self.value
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        Ok(())
    }
}

struct TextCache<'a> {
    title: TextField<'a>,
    artist: TextField<'a>,
    album: TextField<'a>,
    year: TextField<'a>,
    genre: TextField<'a>,
    composer: TextField<'a>,
}

fn build_text_cache<'a>(
    texture_creator: &'a TextureCreator<WindowContext>,
    title_font: &sdl2::ttf::Font,
    body_font: &sdl2::ttf::Font,
    state: &SongState,
    preset: &DisplayPreset,
    layout: &SceneLayout,
) -> Result<TextCache<'a>, String> {
    let panel_x = layout.metadata_region.x() + preset.panel_x;
    let mut panel_y = layout.metadata_region.y() + preset.panel_y;
    let viewport_width = layout
        .metadata_region
        .width()
        .saturating_sub((preset.panel_x as u32).saturating_mul(2));

    let title_line = if state.title.trim().is_empty() {
        "Waiting for music...".to_string()
    } else {
        state.title.clone()
    };

    let artist_line = if state.artist.trim().is_empty() {
        "No track identified yet".to_string()
    } else {
        state.artist.clone()
    };

    let album_line = album_line(state);
    let year_line = release_year_line(state);

    let title = TextField::new(
        texture_creator,
        title_font,
        "Title: ",
        &title_line,
        Color::RGB(255, 255, 255),
        Color::RGB(255, 255, 255),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.title_line_spacing;

    let artist = TextField::new(
        texture_creator,
        body_font,
        "Artist: ",
        &artist_line,
        Color::RGB(170, 170, 170),
        Color::RGB(210, 210, 210),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.body_line_spacing;

    let year_x = panel_x + ((viewport_width as f32) * 0.76) as i32;
    let album_viewport_width = (year_x - panel_x).max(1) as u32;
    let year_viewport_width = (panel_x + viewport_width as i32)
        .saturating_sub(year_x)
        .max(1) as u32;

    let album = TextField::new(
        texture_creator,
        body_font,
        "Album: ",
        &album_line,
        Color::RGB(150, 150, 150),
        Color::RGB(180, 180, 180),
        panel_x,
        panel_y,
        album_viewport_width,
    )?;

    let year = TextField::new(
        texture_creator,
        body_font,
        "Year: ",
        &year_line,
        Color::RGB(150, 150, 150),
        Color::RGB(180, 180, 180),
        year_x,
        panel_y,
        year_viewport_width,
    )?;
    panel_y += preset.detail_line_spacing;

    let composer_x = panel_x + ((viewport_width as f32) * 0.38) as i32;
    let genre_viewport_width = (composer_x - panel_x).max(1) as u32;
    let composer_viewport_width = (panel_x + viewport_width as i32)
        .saturating_sub(composer_x)
        .max(1) as u32;

    let genre = TextField::new(
        texture_creator,
        body_font,
        "Genre: ",
        &state.genre,
        Color::RGB(120, 120, 120),
        Color::RGB(140, 140, 140),
        panel_x,
        panel_y,
        genre_viewport_width,
    )?;

    let composer = TextField::new(
        texture_creator,
        body_font,
        "Composer: ",
        &state.composer,
        Color::RGB(120, 120, 120),
        Color::RGB(140, 140, 140),
        composer_x,
        panel_y,
        composer_viewport_width,
    )?;

    Ok(TextCache {
        title,
        artist,
        album,
        year,
        genre,
        composer,
    })
}

// ==============================================================================
// Visualizer Drawing
// ==============================================================================

fn draw_polyline(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
) -> Result<(), String> {
    if points.len() < 2 {
        return Ok(());
    }

    canvas.set_draw_color(color);

    for pair in points.windows(2) {
        let (x1n, y1n) = pair[0];
        let (x2n, y2n) = pair[1];

        let x1 = x + ((x1n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y1 = y + ((y1n.clamp(0.0, 1.0) * (height as f32)) as i32);
        let x2 = x + ((x2n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y2 = y + ((y2n.clamp(0.0, 1.0) * (height as f32)) as i32);

        canvas.draw_line(Point::new(x1, y1), Point::new(x2, y2))?;
    }

    Ok(())
}

fn draw_polyline_thick(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
    thickness: i32,
) -> Result<(), String> {
    if thickness <= 1 {
        return draw_polyline(canvas, points, x, y, width, height, color);
    }

    canvas.set_draw_color(color);

    let radius = thickness / 2;
    for pair in points.windows(2) {
        let (x1n, y1n) = pair[0];
        let (x2n, y2n) = pair[1];

        let x1 = x + ((x1n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y1 = y + ((y1n.clamp(0.0, 1.0) * (height as f32)) as i32);
        let x2 = x + ((x2n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y2 = y + ((y2n.clamp(0.0, 1.0) * (height as f32)) as i32);

        for offset_x in -radius..=radius {
            for offset_y in -radius..=radius {
                if offset_x.abs() + offset_y.abs() <= radius {
                    canvas.draw_line(
                        Point::new(x1 + offset_x, y1 + offset_y),
                        Point::new(x2 + offset_x, y2 + offset_y),
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn draw_scope_graticule(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let w = width as i32;
    let h = height as i32;

    canvas.set_draw_color(Color::RGB(18, 36, 30));
    for division in 1..10 {
        let line_x = x + (w * division) / 10;
        canvas.draw_line(Point::new(line_x, y), Point::new(line_x, y + h))?;
    }

    for division in 1..8 {
        let line_y = y + (h * division) / 8;
        canvas.draw_line(Point::new(x, line_y), Point::new(x + w, line_y))?;
    }

    canvas.set_draw_color(Color::RGB(38, 78, 62));
    for y_ratio in [0.25_f32, 0.75_f32] {
        let line_y = y + ((height as f32) * y_ratio) as i32;
        canvas.draw_line(Point::new(x, line_y), Point::new(x + w, line_y))?;
    }

    canvas.set_draw_color(Color::RGB(42, 70, 60));
    canvas.draw_rect(Rect::new(x, y, width, height))?;

    Ok(())
}

fn draw_oscilloscope(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    let inset = 10_i32;
    let scope_x = x + inset;
    let scope_y = y + inset;
    let scope_w = width.saturating_sub((inset as u32) * 2).max(1);
    let scope_h = height.saturating_sub((inset as u32) * 2).max(1);

    draw_scope_graticule(canvas, scope_x, scope_y, scope_w, scope_h)?;

    let clip = Rect::new(scope_x, scope_y, scope_w, scope_h);
    canvas.set_clip_rect(Some(clip));

    let trace_result = (|| -> Result<(), String> {
        draw_polyline_thick(
            canvas,
            left_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            dim_color(colors.upper, 0.38),
            5,
        )?;
        draw_polyline_thick(
            canvas,
            right_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            dim_color(colors.lower, 0.38),
            5,
        )?;
        draw_polyline_thick(
            canvas,
            left_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            colors.upper,
            2,
        )?;
        draw_polyline_thick(
            canvas,
            right_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            colors.lower,
            2,
        )?;
        Ok(())
    })();

    canvas.set_clip_rect(None);
    trace_result?;

    Ok(())
}

fn vu_angle(value: f32) -> f32 {
    // Give quiet material useful travel while retaining a small overload area.
    let db = 20.0 * value.max(0.0001).log10();
    let normalized = ((db + 36.0) / 39.0).clamp(0.0, 1.0);
    (-150.0 + normalized * 120.0).to_radians()
}

fn draw_vu_meter(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    rect: Rect,
    level: f32,
    face_texture: Option<&Texture<'_>>,
) -> Result<(), String> {
    let x = rect.x();
    let y = rect.y();
    let w = rect.width() as i32;
    let h = rect.height() as i32;

    let pivot_x = x + w / 2;
    let pivot_y = y + h - 18;
    let radius = ((w as f32 * 0.42).min(h as f32 * 0.82)) as i32;

    if let Some(texture) = face_texture {
        canvas.copy(texture, None, rect)?;
    } else {
        // Lightweight fallback when the photographic meter asset is missing.
        canvas.set_draw_color(Color::RGB(18, 17, 15));
        canvas.fill_rect(rect)?;
        canvas.set_draw_color(Color::RGB(135, 117, 82));
        canvas.draw_rect(Rect::new(
            x + 6,
            y + 6,
            rect.width() - 12,
            rect.height() - 12,
        ))?;
        let face = Rect::new(
            x + 10,
            y + 10,
            rect.width().saturating_sub(20),
            rect.height().saturating_sub(20),
        );
        canvas.set_draw_color(Color::RGB(225, 184, 103));
        canvas.fill_rect(face)?;

        let mut arc = Vec::with_capacity(97);
        for step in 0..=96 {
            let angle = (-150.0 + 120.0 * step as f32 / 96.0).to_radians();
            arc.push(Point::new(
                pivot_x + (angle.cos() * radius as f32) as i32,
                pivot_y + (angle.sin() * radius as f32) as i32,
            ));
        }
        canvas.set_draw_color(Color::RGB(43, 35, 24));
        canvas.draw_lines(arc.as_slice())?;

        for tick in 0..=12 {
            let angle = (-150.0 + tick as f32 * 10.0).to_radians();
            let major = tick % 2 == 0;
            let inner = radius - if major { 20 } else { 12 };
            let color = if tick >= 10 {
                Color::RGB(166, 35, 25)
            } else {
                Color::RGB(48, 39, 27)
            };
            canvas.set_draw_color(color);
            canvas.draw_line(
                Point::new(
                    pivot_x + (angle.cos() * inner as f32) as i32,
                    pivot_y + (angle.sin() * inner as f32) as i32,
                ),
                Point::new(
                    pivot_x + (angle.cos() * radius as f32) as i32,
                    pivot_y + (angle.sin() * radius as f32) as i32,
                ),
            )?;
        }
    }

    // Jewel-style peak lamp: dark red glass at rest, bright core in overload.
    let led_x = x + (w as f32 * 0.83) as i32;
    let led_y = y + (h as f32 * 0.65) as i32;
    let overloaded = level >= 0.70;
    draw_filled_circle(canvas, led_x, led_y, 8, Color::RGB(65, 25, 19))?;
    draw_filled_circle(
        canvas,
        led_x,
        led_y,
        5,
        if overloaded {
            Color::RGB(245, 40, 22)
        } else {
            Color::RGB(105, 28, 20)
        },
    )?;
    if overloaded {
        draw_filled_circle(canvas, led_x - 1, led_y - 1, 2, Color::RGB(255, 176, 105))?;
    }

    let angle = vu_angle(level);
    let needle_length = radius - 9;
    let tip = Point::new(
        pivot_x + (angle.cos() * needle_length as f32) as i32,
        pivot_y + (angle.sin() * needle_length as f32) as i32,
    );

    // Offset shadow, then a tapered-looking warm black needle.
    canvas.set_draw_color(Color::RGBA(78, 55, 30, 100));
    canvas.draw_line(
        Point::new(pivot_x + 3, pivot_y + 4),
        Point::new(tip.x + 3, tip.y + 4),
    )?;
    canvas.set_draw_color(Color::RGB(42, 29, 18));
    for offset in -1..=1 {
        canvas.draw_line(
            Point::new(pivot_x + offset, pivot_y),
            Point::new(tip.x + offset, tip.y),
        )?;
    }

    draw_filled_circle(canvas, pivot_x, pivot_y, 10, Color::RGB(45, 40, 33))?;
    draw_filled_circle(canvas, pivot_x, pivot_y, 5, Color::RGB(151, 126, 78))?;

    Ok(())
}

fn draw_analog_vu(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    level: f32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    face_texture: Option<&Texture<'_>>,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    let gap = (width / 40).max(10);
    let meter_width = width.saturating_sub(gap) / 2;
    let meter_height = height.saturating_sub(8);
    draw_vu_meter(
        canvas,
        Rect::new(x, y + 4, meter_width, meter_height),
        level,
        face_texture,
    )?;
    draw_vu_meter(
        canvas,
        Rect::new(
            x + meter_width as i32 + gap as i32,
            y + 4,
            meter_width,
            meter_height,
        ),
        level,
        face_texture,
    )
}

fn draw_spectrum(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    upper_bins: &[f32],
    lower_bins: &[f32],
    upper_peaks: &[f32],
    lower_peaks: &[f32],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    bar_gap: u32,
    spectrum: &RuntimeSpectrumSettings,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    if upper_bins.is_empty() || lower_bins.is_empty() {
        return Ok(());
    }

    let half_h = height / 2;
    if half_h == 0 {
        return Ok(());
    }

    let count = upper_bins.len().min(lower_bins.len()) as u32;
    if count == 0 {
        return Ok(());
    }

    if spectrum.segmented() {
        return draw_segmented_spectrum(
            canvas,
            ctx,
            colors,
            upper_bins,
            upper_peaks,
            x,
            y,
            width,
            height,
            bar_gap,
            spectrum,
        );
    }

    let total_gap = bar_gap.saturating_mul(count.saturating_sub(1));
    let bar_w = (width.saturating_sub(total_gap) / count).max(1);
    let top_only = spectrum.top_only();
    let upper_h = if top_only { height } else { half_h };
    let baseline_y = if top_only {
        y + (height as i32)
    } else {
        y + (half_h as i32)
    };

    if !top_only {
        canvas.set_draw_color(Color::RGB(40, 40, 40));
        canvas.draw_line(
            Point::new(x, baseline_y),
            Point::new(x + (width as i32), baseline_y),
        )?;
    }

    for (i, value) in upper_bins.iter().enumerate() {
        let i = i as u32;
        let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
        let bar_h = ((*value).clamp(0.0, 1.0) * (upper_h as f32)) as u32;

        canvas.set_draw_color(palette_color_at(
            &colors.palette,
            i as usize,
            count as usize,
        ));

        if let Some(rect) = spectrum_bar_rect(
            bar_x,
            baseline_y,
            bar_w,
            bar_h,
            true,
            &spectrum.render_style,
            ctx.config.visualizer.spectrum.top_only_height_ratio,
        ) {
            canvas.fill_rect(rect)?;
        }
    }

    if !top_only {
        for (i, value) in lower_bins.iter().enumerate() {
            let i = i as u32;
            let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
            let bar_h = ((*value).clamp(0.0, 1.0) * (half_h as f32)) as u32;

            canvas.set_draw_color(palette_color_at(
                &colors.palette,
                count.saturating_sub(1).saturating_sub(i) as usize,
                count as usize,
            ));

            if let Some(rect) = spectrum_bar_rect(
                bar_x,
                y + (half_h as i32),
                bar_w,
                bar_h,
                false,
                &spectrum.render_style,
                ctx.config.visualizer.spectrum.top_only_height_ratio,
            ) {
                canvas.fill_rect(rect)?;
            }
        }
    }

    if ctx.config.visualizer.peaks.enabled {
        let peak_marker_h = ctx.config.visualizer.peaks.drop_pixels.max(1).min(half_h);
        let peak_color = parse_hex_color(
            &ctx.config.visualizer.peaks.color,
            Color::RGB(255, 255, 255),
        );

        for (i, value) in upper_peaks.iter().take(count as usize).enumerate() {
            let i = i as u32;
            let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
            let peak_h = ((*value).clamp(0.0, 1.0) * (upper_h as f32)) as u32;

            if peak_h == 0 {
                continue;
            }

            canvas.set_draw_color(if ctx.config.visualizer.peaks.use_bar_color {
                palette_color_at(&colors.palette, i as usize, count as usize)
            } else {
                peak_color
            });

            let marker_y = baseline_y - (peak_h as i32);
            canvas.fill_rect(Rect::new(bar_x, marker_y, bar_w, peak_marker_h))?;
        }

        if !top_only {
            for (i, value) in lower_peaks.iter().take(count as usize).enumerate() {
                let i = i as u32;
                let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
                let peak_h = ((*value).clamp(0.0, 1.0) * (half_h as f32)) as u32;

                if peak_h == 0 {
                    continue;
                }

                canvas.set_draw_color(if ctx.config.visualizer.peaks.use_bar_color {
                    palette_color_at(
                        &colors.palette,
                        count.saturating_sub(1).saturating_sub(i) as usize,
                        count as usize,
                    )
                } else {
                    peak_color
                });

                let marker_y = y + (half_h as i32) + (peak_h as i32) - (peak_marker_h as i32);
                canvas.fill_rect(Rect::new(bar_x, marker_y, bar_w, peak_marker_h))?;
            }
        }
    }

    Ok(())
}

fn dim_segment_color(color: Color, alpha: u8) -> Color {
    Color::RGBA(
        (color.r as f32 * 0.45).round() as u8,
        (color.g as f32 * 0.45).round() as u8,
        (color.b as f32 * 0.45).round() as u8,
        alpha,
    )
}

fn segmented_row_rect(
    bar_x: i32,
    bottom_y: i32,
    bar_w: u32,
    row: u32,
    segment_h: u32,
    row_step: f32,
) -> Rect {
    let row_bottom = bottom_y - ((row as f32) * row_step).round() as i32;
    Rect::new(bar_x, row_bottom - segment_h as i32, bar_w, segment_h)
}

fn segmented_row_step(height: u32, rows: u32, segment_h: u32, segment_gap: u32) -> f32 {
    if rows <= 1 {
        return segment_h.max(1) as f32;
    }

    let fit_step = height.saturating_sub(segment_h) as f32 / rows.saturating_sub(1) as f32;
    let min_step = segment_h.saturating_add(segment_gap) as f32;
    if min_step * rows.saturating_sub(1) as f32 + segment_h as f32 <= height as f32 {
        fit_step.max(min_step)
    } else {
        fit_step.max(1.0)
    }
}

fn draw_segmented_spectrum(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    bins: &[f32],
    peaks: &[f32],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    _bar_gap: u32,
    spectrum: &RuntimeSpectrumSettings,
) -> Result<(), String> {
    let count = bins.len() as u32;
    if count == 0 || height == 0 {
        return Ok(());
    }

    let column_gap = spectrum.segment_column_gap;
    let total_gap = column_gap.saturating_mul(count.saturating_sub(1));
    let bar_w = (width.saturating_sub(total_gap) / count).max(1);
    let rows = spectrum.segment_rows.clamp(4, 96);
    let segment_gap = spectrum.segment_gap.min(height / rows.max(1));
    let total_row_gap = segment_gap.saturating_mul(rows.saturating_sub(1));
    let max_segment_h = height.saturating_sub(total_row_gap) / rows;
    let segment_h = spectrum.segment_height.clamp(1, max_segment_h.max(1));
    if segment_h == 0 {
        return Ok(());
    }

    canvas.set_blend_mode(BlendMode::Blend);
    let bottom_y = y + height as i32;
    let row_step = segmented_row_step(height, rows, segment_h, segment_gap);
    let inactive_alpha = ctx.config.visualizer.spectrum.segment_inactive_alpha;
    let draw_inactive = spectrum.segment_inactive;

    for (i, value) in bins.iter().enumerate() {
        let i_u32 = i as u32;
        let bar_x = x + ((i_u32 * (bar_w + column_gap)) as i32);
        let color = palette_color_at(&colors.palette, i, count as usize);
        let active_rows = ((*value).clamp(0.0, 1.0) * rows as f32).ceil() as u32;

        if draw_inactive {
            canvas.set_draw_color(dim_segment_color(color, inactive_alpha));
            for row in 0..rows {
                canvas.fill_rect(segmented_row_rect(
                    bar_x, bottom_y, bar_w, row, segment_h, row_step,
                ))?;
            }
        }

        canvas.set_draw_color(color);
        for row in 0..active_rows.min(rows) {
            canvas.fill_rect(segmented_row_rect(
                bar_x, bottom_y, bar_w, row, segment_h, row_step,
            ))?;
        }
    }

    if ctx.config.visualizer.peaks.enabled {
        let peak_color = parse_hex_color(
            &ctx.config.visualizer.peaks.color,
            Color::RGB(255, 255, 255),
        );

        for (i, value) in peaks.iter().take(count as usize).enumerate() {
            let peak_row = ((*value).clamp(0.0, 1.0) * rows as f32).ceil() as u32;
            if peak_row == 0 {
                continue;
            }

            let i_u32 = i as u32;
            let bar_x = x + ((i_u32 * (bar_w + column_gap)) as i32);
            canvas.set_draw_color(if ctx.config.visualizer.peaks.use_bar_color {
                palette_color_at(&colors.palette, i, count as usize)
            } else {
                peak_color
            });
            canvas.fill_rect(segmented_row_rect(
                bar_x,
                bottom_y,
                bar_w,
                peak_row.saturating_sub(1).min(rows - 1),
                segment_h,
                row_step,
            ))?;
        }
    }

    Ok(())
}

fn spectrum_bar_rect(
    bar_x: i32,
    baseline_y: i32,
    bar_w: u32,
    bar_h: u32,
    extends_up: bool,
    render_style: &str,
    top_only_height_ratio: f32,
) -> Option<Rect> {
    if bar_h == 0 {
        return None;
    }

    let top_only = render_style.eq_ignore_ascii_case("top_only");
    let visible_h = if top_only {
        ((bar_h as f32) * top_only_height_ratio.clamp(0.0, 1.0))
            .ceil()
            .max(1.0) as u32
    } else {
        bar_h
    };

    let rect_y = if extends_up {
        baseline_y - (bar_h as i32)
    } else if top_only {
        baseline_y + (bar_h as i32) - (visible_h as i32)
    } else {
        baseline_y
    };

    Some(Rect::new(bar_x, rect_y, bar_w, visible_h))
}

fn draw_visualizer(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    mode: VisualizerMode,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    upper_bins: &[f32],
    lower_bins: &[f32],
    upper_peaks: &[f32],
    lower_peaks: &[f32],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    bar_gap: u32,
    meter_level: f32,
    vu_face_texture: Option<&Texture<'_>>,
    spectrum: &RuntimeSpectrumSettings,
) -> Result<(), String> {
    match mode {
        VisualizerMode::None => Ok(()),
        VisualizerMode::Oscilloscope => draw_oscilloscope(
            canvas,
            ctx,
            colors,
            left_points,
            right_points,
            x,
            y,
            width,
            height,
        ),
        VisualizerMode::AnalogVu => draw_analog_vu(
            canvas,
            ctx,
            meter_level,
            x,
            y,
            width,
            height,
            vu_face_texture,
        ),
        VisualizerMode::Spectrum => draw_spectrum(
            canvas,
            ctx,
            colors,
            upper_bins,
            lower_bins,
            upper_peaks,
            lower_peaks,
            x,
            y,
            width,
            height,
            bar_gap,
            spectrum,
        ),
    }
}

// ==============================================================================
// Static Scene Drawing
// ==============================================================================

fn compute_artwork_rect(query: TextureQuery, region: Rect) -> Rect {
    let art_w = query.width as f32;
    let art_h = query.height as f32;

    let padding = 24.0;
    let max_w = (region.width() as f32) - padding * 2.0;
    let max_h = (region.height() as f32) - padding * 2.0;

    let scale = f32::min(max_w / art_w, max_h / art_h);
    let draw_w = (art_w * scale) as u32;
    let draw_h = (art_h * scale) as u32;

    let x = region.x() + ((region.width() - draw_w) / 2) as i32;
    let y = region.y() + ((region.height() - draw_h) / 2) as i32;

    Rect::new(x, y, draw_w, draw_h)
}

fn compute_record_rect(region: Rect) -> Rect {
    let padding = 24u32;
    let diameter = region
        .width()
        .saturating_sub(padding * 2)
        .min(region.height().saturating_sub(padding * 2));
    Rect::new(
        region.x() + ((region.width() - diameter) / 2) as i32,
        region.y() + ((region.height() - diameter) / 2) as i32,
        diameter,
        diameter,
    )
}

fn draw_filled_circle(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    center_x: i32,
    center_y: i32,
    radius: i32,
    color: Color,
) -> Result<(), String> {
    canvas.set_draw_color(color);
    for y in -radius..=radius {
        let half_width = ((radius * radius - y * y) as f32).sqrt() as i32;
        canvas.draw_line(
            Point::new(center_x - half_width, center_y + y),
            Point::new(center_x + half_width, center_y + y),
        )?;
    }
    Ok(())
}

fn draw_circle_outline(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    center_x: i32,
    center_y: i32,
    radius: i32,
    color: Color,
) -> Result<(), String> {
    const SEGMENTS: usize = 96;
    let mut points = Vec::with_capacity(SEGMENTS + 1);
    for segment in 0..=SEGMENTS {
        let angle = (segment as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        points.push(Point::new(
            center_x + (angle.cos() * radius as f32).round() as i32,
            center_y + (angle.sin() * radius as f32).round() as i32,
        ));
    }
    canvas.set_draw_color(color);
    canvas.draw_lines(points.as_slice())
}

fn draw_spiral_groove(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    center_x: i32,
    center_y: i32,
    outer_radius: i32,
    inner_radius: i32,
    color: Color,
) -> Result<(), String> {
    // One continuous Archimedean spiral approximates the groove cut into an
    // LP. Seventy-two turns keep the groove bed dense without overwhelming
    // the Raspberry Pi renderer.
    const TURNS: usize = 72;
    const POINTS_PER_TURN: usize = 48;
    let point_count = TURNS * POINTS_PER_TURN;
    let mut points = Vec::with_capacity(point_count + 1);

    for point in 0..=point_count {
        let progress = point as f32 / point_count as f32;
        let radius = outer_radius as f32 + (inner_radius - outer_radius) as f32 * progress;
        let angle = progress * TURNS as f32 * std::f32::consts::TAU;
        points.push(Point::new(
            center_x + (angle.cos() * radius).round() as i32,
            center_y + (angle.sin() * radius).round() as i32,
        ));
    }

    canvas.set_draw_color(color);
    canvas.draw_lines(points.as_slice())
}

fn draw_vinyl_record(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    target: Rect,
    opacity: f32,
) -> Result<(), String> {
    let opacity = opacity.clamp(0.0, 1.0);
    let dim = |value: u8| ((value as f32 * opacity).round()) as u8;
    let center_x = target.x() + target.width() as i32 / 2;
    let center_y = target.y() + target.height() as i32 / 2;
    let radius = target.width() as i32 / 2;
    draw_filled_circle(
        canvas,
        center_x,
        center_y,
        radius,
        Color::RGB(dim(12), dim(12), dim(14)),
    )?;

    // A real record has one densely packed continuous spiral groove rather
    // than a small stack of widely spaced rings.
    let label_radius = radius / 6;
    let inner_groove_radius = label_radius + (radius / 28).max(2);
    let outer_groove_radius = radius - 5;
    draw_spiral_groove(
        canvas,
        center_x,
        center_y,
        outer_groove_radius,
        inner_groove_radius,
        Color::RGB(dim(38), dim(38), dim(42)),
    )?;

    // Four wider-pitch bands divide the side into five plausible tracks.
    // Darkening the groove bed and catching one edge makes each break visible
    // without turning the record back into a bullseye.
    let groove_span = outer_groove_radius - inner_groove_radius;
    for track in 1..5 {
        let break_radius = inner_groove_radius + (groove_span * track / 5);
        for offset in -2..=2 {
            draw_circle_outline(
                canvas,
                center_x,
                center_y,
                break_radius + offset,
                Color::RGB(dim(14), dim(14), dim(16)),
            )?;
        }
        draw_circle_outline(
            canvas,
            center_x,
            center_y,
            break_radius + 3,
            Color::RGB(dim(30), dim(30), dim(34)),
        )?;
    }

    // The smooth runout area and raised outer lip catch a little more light.
    draw_circle_outline(
        canvas,
        center_x,
        center_y,
        label_radius + (radius / 40).max(2),
        Color::RGB(dim(31), dim(31), dim(35)),
    )?;
    draw_circle_outline(
        canvas,
        center_x,
        center_y,
        radius - 1,
        Color::RGB(dim(48), dim(48), dim(52)),
    )
}

fn draw_circular_artwork(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture: &Texture<'_>,
    target: Rect,
) -> Result<(), String> {
    let query = texture.query();
    let source_size = query.width.min(query.height);
    let source_x = (query.width - source_size) / 2;
    let source_y = (query.height - source_size) / 2;
    let radius = target.width() as i32 / 2;
    let center_x = target.x() + radius;
    let center_y = target.y() + radius;

    for y in -radius..radius {
        let half_width = ((radius * radius - y * y) as f32).sqrt() as i32;
        let destination_width = (half_width * 2).max(1) as u32;
        let source_row = (((y + radius) as f32 / (radius * 2) as f32) * source_size as f32) as u32;
        let source_half_width =
            ((half_width as f32 / radius as f32) * source_size as f32 / 2.0) as u32;
        let source_center = source_x + source_size / 2;
        let source_width = (source_half_width * 2).max(1);

        canvas.copy(
            texture,
            Rect::new(
                source_center.saturating_sub(source_half_width) as i32,
                (source_y + source_row.min(source_size - 1)) as i32,
                source_width.min(source_size),
                1,
            ),
            Rect::new(center_x - half_width, center_y + y, destination_width, 1),
        )?;
    }
    Ok(())
}

struct StaticSceneCache<'a> {
    version: u64,
    text: TextCache<'a>,
    artwork_rect: Option<Rect>,
}

impl<'a> StaticSceneCache<'a> {
    fn draw_static_scene(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        ctx: &AppContext,
        layout: &SceneLayout,
    ) -> Result<(), String> {
        canvas.set_draw_color(canvas_background_color(ctx));
        canvas.clear();

        canvas.set_draw_color(artwork_background_color(ctx));
        canvas.fill_rect(layout.artwork_region)?;

        canvas.set_draw_color(metadata_background_color(ctx));
        canvas.fill_rect(layout.metadata_region)?;

        if layout.visualizer_region != layout.metadata_region {
            canvas.set_draw_color(visualizer_background_color(ctx));
            canvas.fill_rect(layout.visualizer_region)?;
        }

        Ok(())
    }

    fn draw_text(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        self.text
            .title
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .artist
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .album
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .year
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .genre
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .composer
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;

        Ok(())
    }
}

// ==============================================================================
// Runtime Helpers
// ==============================================================================

fn visualizer_mode_from_config(mode: &str) -> VisualizerMode {
    match mode.to_ascii_lowercase().as_str() {
        "oscilloscope" => VisualizerMode::Oscilloscope,
        "spectrum" => VisualizerMode::Spectrum,
        "analog_vu" => VisualizerMode::AnalogVu,
        _ => VisualizerMode::None,
    }
}

fn cycle_option(current: &str, options: &[&str], direction: i32) -> String {
    let index = options
        .iter()
        .position(|option| option.eq_ignore_ascii_case(current))
        .unwrap_or(0) as i32;
    let len = options.len() as i32;
    options[(index + direction).rem_euclid(len) as usize].to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DisplayRotation {
    Normal,
    Clockwise,
    Inverted,
    CounterClockwise,
}

impl DisplayRotation {
    fn parse(rotation: &str) -> Option<Self> {
        match rotation.trim().to_ascii_lowercase().as_str() {
            "normal" | "0" => Some(Self::Normal),
            "clockwise" | "right" | "90" => Some(Self::Clockwise),
            "inverted" | "upside_down" | "180" => Some(Self::Inverted),
            "counter_clockwise" | "left" | "270" => Some(Self::CounterClockwise),
            _ => None,
        }
    }

    fn angle(self) -> f64 {
        match self {
            Self::Normal => 0.0,
            Self::Clockwise => 90.0,
            Self::Inverted => 180.0,
            Self::CounterClockwise => 270.0,
        }
    }

    fn swaps_dimensions(self) -> bool {
        matches!(self, Self::Clockwise | Self::CounterClockwise)
    }

    fn canonical(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Clockwise => "clockwise",
            Self::Inverted => "inverted",
            Self::CounterClockwise => "counter_clockwise",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayRotation, metadata_font_theme_name, scene_layout, segmented_row_rect,
        segmented_row_step, selected_font_theme_name,
    };
    use crate::config::DisplayPreset;

    #[test]
    fn display_rotation_accepts_canonical_values_and_aliases() {
        let cases = [
            ("normal", DisplayRotation::Normal, 0.0, false),
            ("0", DisplayRotation::Normal, 0.0, false),
            ("clockwise", DisplayRotation::Clockwise, 90.0, true),
            ("right", DisplayRotation::Clockwise, 90.0, true),
            ("90", DisplayRotation::Clockwise, 90.0, true),
            ("inverted", DisplayRotation::Inverted, 180.0, false),
            ("upside_down", DisplayRotation::Inverted, 180.0, false),
            ("180", DisplayRotation::Inverted, 180.0, false),
            (
                "counter_clockwise",
                DisplayRotation::CounterClockwise,
                270.0,
                true,
            ),
            ("left", DisplayRotation::CounterClockwise, 270.0, true),
            ("270", DisplayRotation::CounterClockwise, 270.0, true),
        ];

        for (input, expected, angle, swaps_dimensions) in cases {
            let rotation = DisplayRotation::parse(input).unwrap();
            assert_eq!(rotation, expected);
            assert_eq!(rotation.angle(), angle);
            assert_eq!(rotation.swaps_dimensions(), swaps_dimensions);
        }
    }

    #[test]
    fn display_rotation_rejects_unknown_values() {
        assert_eq!(DisplayRotation::parse("sideways"), None);
    }

    #[test]
    fn metadata_font_theme_prefers_genre_over_release_year() {
        assert_eq!(
            metadata_font_theme_name("Rock", "2019-03-01", "simple"),
            "grungy"
        );
        assert_eq!(
            metadata_font_theme_name("Latin Pop", "1984", "simple"),
            "scripted"
        );
    }

    #[test]
    fn metadata_font_theme_covers_representative_rules() {
        let cases = [
            ("Electronic", "2024", "techy"),
            ("Alternative Rock", "2024", "grungy"),
            ("Film Score", "2024", "fantasy"),
            ("Singer-Songwriter", "2024", "scripted"),
            ("Jazz", "2024", "retro"),
            ("Hip-Hop", "1974", "modern"),
            ("Unknown", "1974", "retro"),
            ("Unknown", "1984", "techy"),
            ("Unknown", "1994", "grungy"),
            ("Unknown", "2004", "modern"),
            ("Unknown", "Unknown", "simple"),
        ];

        for (genre, released, expected) in cases {
            assert_eq!(
                metadata_font_theme_name(genre, released, "simple"),
                expected
            );
        }
    }

    #[test]
    fn selected_font_theme_honors_fixed_mode() {
        let (theme, invalid_mode) =
            selected_font_theme_name(" fixed ", " Scripted ", "Pop", "2024", "simple");

        assert_eq!(theme, "scripted");
        assert!(!invalid_mode);
    }

    #[test]
    fn selected_font_theme_recovers_from_invalid_mode_with_metadata() {
        let (theme, invalid_mode) =
            selected_font_theme_name("analog_vu", "scripted", "Pop", "1998", "simple");

        assert_eq!(theme, "modern");
        assert!(invalid_mode);
    }

    #[test]
    fn segmented_rows_stack_upward_with_gaps() {
        let first = segmented_row_rect(10, 100, 20, 0, 4, 6.0);
        let second = segmented_row_rect(10, 100, 20, 1, 4, 6.0);

        assert_eq!(first.x(), 10);
        assert_eq!(first.y(), 96);
        assert_eq!(first.width(), 20);
        assert_eq!(first.height(), 4);
        assert_eq!(second.y(), 90);
        assert_eq!(first.y() - (second.y() + second.height() as i32), 2);
    }

    #[test]
    fn segmented_rows_span_full_region_with_thin_segments() {
        let row_step = segmented_row_step(100, 24, 3, 2);
        let bottom_y = 110;
        let top_row = segmented_row_rect(0, bottom_y, 10, 23, 3, row_step);
        let bottom_row = segmented_row_rect(0, bottom_y, 10, 0, 3, row_step);

        assert_eq!(top_row.y(), 10);
        assert_eq!(bottom_row.y() + bottom_row.height() as i32, 110);
    }

    #[test]
    fn landscape_layout_places_artwork_right_and_visualizer_under_metadata() {
        let preset = DisplayPreset {
            width: 1920,
            height: 1080,
            top_panel_ratio: 0.72,
            panel_x: 40,
            panel_y: 28,
            title_line_spacing: 46,
            body_line_spacing: 34,
            detail_line_spacing: 40,
        };

        let layout = scene_layout(&preset);

        assert!(layout.artwork_region.x() > layout.metadata_region.x());
        assert_eq!(layout.metadata_region.x(), layout.visualizer_region.x());
        assert_eq!(
            layout.metadata_region.width(),
            layout.visualizer_region.width()
        );
        assert!(layout.visualizer_region.y() > layout.metadata_region.y());
    }

    #[test]
    fn portrait_layout_preserves_top_artwork_region() {
        let preset = DisplayPreset {
            width: 1080,
            height: 1920,
            top_panel_ratio: 0.66,
            panel_x: 48,
            panel_y: 36,
            title_line_spacing: 52,
            body_line_spacing: 38,
            detail_line_spacing: 44,
        };

        let layout = scene_layout(&preset);

        assert_eq!(layout.artwork_region.x(), 0);
        assert_eq!(layout.artwork_region.y(), 0);
        assert_eq!(layout.artwork_region.width(), preset.width);
        assert!(layout.metadata_region.y() > layout.artwork_region.y());
    }
}

fn save_display_modes(
    path: &str,
    artwork_mode: &str,
    visualizer_mode: &str,
    spectrum: &RuntimeSpectrumSettings,
    visualizer_gain: f32,
    display_orientation: &str,
    display_rotation: &str,
) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("Failed to read {path}: {e}"))?;
    let mut document = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("Failed to parse {path}: {e}"))?;
    document["artwork"]["mode"] = toml_edit::value(artwork_mode);
    document["visualizer"]["mode"] = toml_edit::value(visualizer_mode);
    document["visualizer"]["spectrum"]["render_style"] = toml_edit::value(&spectrum.render_style);
    document["visualizer"]["spectrum"]["segment_rows"] =
        toml_edit::value(spectrum.segment_rows as i64);
    document["visualizer"]["spectrum"]["segment_height"] =
        toml_edit::value(spectrum.segment_height as i64);
    document["visualizer"]["spectrum"]["segment_gap"] =
        toml_edit::value(spectrum.segment_gap as i64);
    document["visualizer"]["spectrum"]["segment_column_gap"] =
        toml_edit::value(spectrum.segment_column_gap as i64);
    document["visualizer"]["spectrum"]["segment_inactive"] =
        toml_edit::value(spectrum.segment_inactive);
    document["visualizer"]["gain"] = toml_edit::value(visualizer_gain as f64);
    document["display"]["orientation"] = toml_edit::value(display_orientation);
    document["display"]["rotation"] = toml_edit::value(display_rotation);

    let backup = format!("{path}.bak");
    let temporary = format!("{path}.tmp");
    fs::copy(path, &backup).map_err(|e| format!("Failed to create {backup}: {e}"))?;
    fs::write(&temporary, document.to_string())
        .map_err(|e| format!("Failed to write {temporary}: {e}"))?;
    fs::rename(&temporary, path).map_err(|e| format!("Failed to replace {path}: {e}"))
}

fn draw_settings_text(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture_creator: &TextureCreator<WindowContext>,
    font: &sdl2::ttf::Font,
    text: &str,
    color: Color,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let surface = font
        .render(text)
        .blended(color)
        .map_err(|e| e.to_string())?;
    let texture = texture_creator
        .create_texture_from_surface(&surface)
        .map_err(|e| e.to_string())?;
    let query = texture.query();
    canvas.copy(&texture, None, Rect::new(x, y, query.width, query.height))
}

fn draw_settings_overlay(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture_creator: &TextureCreator<WindowContext>,
    font: &sdl2::ttf::Font,
    artwork_mode: &str,
    visualizer_mode: &str,
    spectrum: &RuntimeSpectrumSettings,
    visualizer_gain: f32,
    display_orientation: &str,
    display_rotation: &str,
    selected: usize,
    status: &str,
) -> Result<(), String> {
    let (canvas_w, canvas_h) = canvas.output_size().map_err(|e| e.to_string())?;
    let rows = settings_rows(visualizer_mode, spectrum);
    let panel_w = canvas_w.saturating_sub(80).min(760);
    let row_spacing = 36i32;
    let panel_h = (190 + rows.len() as u32 * row_spacing as u32)
        .min(canvas_h.saturating_sub(80).max(1));
    let panel_x = ((canvas_w - panel_w) / 2) as i32;
    let panel_y = ((canvas_h - panel_h) / 2) as i32;

    canvas.set_blend_mode(BlendMode::Blend);
    canvas.set_draw_color(Color::RGBA(8, 8, 10, 235));
    canvas.fill_rect(Rect::new(panel_x, panel_y, panel_w, panel_h))?;
    canvas.set_draw_color(Color::RGB(190, 145, 63));
    canvas.draw_rect(Rect::new(panel_x, panel_y, panel_w, panel_h))?;

    draw_settings_text(
        canvas,
        texture_creator,
        font,
        "SONGART SETTINGS",
        Color::RGB(244, 205, 125),
        panel_x + 32,
        panel_y + 25,
    )?;

    let filled = ((visualizer_gain / 8.0) * 16.0).round().clamp(1.0, 16.0) as usize;
    let slider = format!(
        "[{}{}] {:.2}",
        "=".repeat(filled),
        "-".repeat(16 - filled),
        visualizer_gain,
    );
    let lines: Vec<String> = rows
        .iter()
        .map(|row| match row {
            SettingsRow::Artwork => format!("Artwork       < {} >", artwork_mode),
            SettingsRow::Visualizer => format!("Visualizer    < {} >", visualizer_mode),
            SettingsRow::SpectrumStyle => format!("Spectrum      < {} >", spectrum.render_style),
            SettingsRow::SegmentRows => format!("Segments      < {} >", spectrum.segment_rows),
            SettingsRow::SegmentHeight => format!("Segment H     < {} >", spectrum.segment_height),
            SettingsRow::SegmentGap => format!("Row Gap       < {} >", spectrum.segment_gap),
            SettingsRow::SegmentColumnGap => {
                format!("Column Gap    < {} >", spectrum.segment_column_gap)
            }
            SettingsRow::SegmentInactive => format!(
                "Inactive LEDs < {} >",
                if spectrum.segment_inactive {
                    "on"
                } else {
                    "off"
                }
            ),
            SettingsRow::Sensitivity => format!("Sensitivity   {}", slider),
            SettingsRow::Orientation => format!("Orientation   < {} >", display_orientation),
            SettingsRow::Rotation => format!("Rotation      < {} >", display_rotation),
        })
        .collect();

    for (index, line) in lines.iter().enumerate() {
        let row_y = panel_y + 58 + index as i32 * row_spacing;
        if selected == index {
            canvas.set_draw_color(Color::RGBA(104, 72, 28, 180));
            canvas.fill_rect(Rect::new(
                panel_x + 22,
                row_y - 6,
                panel_w - 44,
                row_spacing as u32,
            ))?;
        }
        draw_settings_text(
            canvas,
            texture_creator,
            font,
            line,
            if selected == index {
                Color::RGB(255, 225, 161)
            } else {
                Color::RGB(205, 196, 178)
            },
            panel_x + 38,
            row_y,
        )?;
    }

    draw_settings_text(
        canvas,
        texture_creator,
        font,
        "Up/Down select   Left/Right change   Enter apply",
        Color::RGB(155, 150, 140),
        panel_x + 32,
        panel_y + panel_h as i32 - 105,
    )?;
    draw_settings_text(
        canvas,
        texture_creator,
        font,
        "Saved orientation/rotation take full effect after restart",
        Color::RGB(155, 150, 140),
        panel_x + 32,
        panel_y + panel_h as i32 - 73,
    )?;
    draw_settings_text(
        canvas,
        texture_creator,
        font,
        "S save   Esc cancel",
        Color::RGB(155, 150, 140),
        panel_x + 32,
        panel_y + panel_h as i32 - 41,
    )?;
    if !status.is_empty() {
        draw_settings_text(
            canvas,
            texture_creator,
            font,
            status,
            Color::RGB(118, 220, 142),
            panel_x + panel_w as i32 - 210,
            panel_y + panel_h as i32 - 41,
        )?;
    }
    Ok(())
}

fn update_smoothed_bins(smoothed: &mut [f32], raw: &[f32], rise: f32, fall: f32) {
    for (i, value) in raw.iter().enumerate() {
        if i < smoothed.len() {
            let current = smoothed[i];
            smoothed[i] = if *value > current {
                current * rise + *value * (1.0 - rise)
            } else {
                current * fall + *value * (1.0 - fall)
            };
        }
    }
}

fn update_spectrum_peaks(
    peaks: &mut Vec<f32>,
    current: &[f32],
    drop_amount: f32,
    should_drop: bool,
) -> bool {
    if peaks.len() != current.len() {
        peaks.resize(current.len(), 0.0);
    }

    let mut rose = false;

    if should_drop {
        for peak in peaks.iter_mut() {
            *peak = (*peak - drop_amount).max(0.0);
        }
    }

    for (peak, value) in peaks.iter_mut().zip(current.iter()) {
        let value = value.clamp(0.0, 1.0);
        if value > *peak {
            *peak = value;
            rose = true;
        }
    }

    rose
}

fn log_visualizer_debug(
    ctx: &AppContext,
    audio_len: usize,
    sample_len: usize,
    live_level: f32,
    smoothed_upper_bins: &[f32],
) {
    let smooth_max_bin = smoothed_upper_bins.iter().copied().fold(0.0f32, f32::max);

    let smooth_avg_bin = if smoothed_upper_bins.is_empty() {
        0.0
    } else {
        smoothed_upper_bins.iter().sum::<f32>() / (smoothed_upper_bins.len() as f32)
    };

    log_debug(
        ctx,
        &format!(
            "Visualizer debug: audio_len={} sample_len={} level={:.3} bins={} smooth_max={:.3} smooth_avg={:.3} upper0={:.3} upper8={:.3} upper16={:.3}",
            audio_len,
            sample_len,
            live_level,
            smoothed_upper_bins.len(),
            smooth_max_bin,
            smooth_avg_bin,
            smoothed_upper_bins.get(0).copied().unwrap_or(0.0),
            smoothed_upper_bins.get(8).copied().unwrap_or(0.0),
            smoothed_upper_bins.get(16).copied().unwrap_or(0.0)
        ),
    );
}

// ==============================================================================
// Display Loop
// ==============================================================================

pub fn run_display_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _image_ctx = sdl2::image::init(InitFlag::JPG | InitFlag::PNG)?;
    let ttf_ctx = sdl2::ttf::init().map_err(|e| e.to_string())?;

    log_info(
        &ctx,
        &format!("SDL video driver: {}", video.current_video_driver()),
    );

    let preset = selected_display_preset(&ctx)
        .ok_or_else(|| format!("Unknown display preset: {}", ctx.config.display.orientation))?;
    let configured_rotation =
        DisplayRotation::parse(&ctx.config.display.rotation).unwrap_or(DisplayRotation::Normal);
    let window_w = if configured_rotation.swaps_dimensions() {
        preset.height
    } else {
        preset.width
    };
    let window_h = if configured_rotation.swaps_dimensions() {
        preset.width
    } else {
        preset.height
    };

    let mut window_builder = video.window(&ctx.config.display.window_title, window_w, window_h);
    window_builder.position_centered();

    let window = if ctx.config.display.fullscreen {
        window_builder
            .fullscreen_desktop()
            .build()
            .map_err(|e| e.to_string())?
    } else {
        window_builder.build().map_err(|e| e.to_string())?
    };

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .build()
        .map_err(|e| e.to_string())?;

    let info = canvas.info();
    log_info(
        &ctx,
        &format!("SDL renderer: name='{}' flags={:?}", info.name, info.flags),
    );

    let texture_creator = canvas.texture_creator();

    let initial_state = SongState::default();
    let (title_font_path, body_font_path, title_font_size, body_font_size, selected_theme) =
        selected_fonts(&ctx, &initial_state);

    log_info(
        &ctx,
        &format!(
            "Selected font theme '{}' title_font='{}' body_font='{}' title_size={} body_size={}",
            selected_theme, title_font_path, body_font_path, title_font_size, body_font_size
        ),
    );

    let mut title_font = ttf_ctx
        .load_font(title_font_path, title_font_size)
        .map_err(|e| format!("Failed to load title font from {}: {e}", title_font_path))?;

    let mut body_font = ttf_ctx
        .load_font(body_font_path, body_font_size)
        .map_err(|e| format!("Failed to load body font from {}: {e}", body_font_path))?;

    // Settings remain readable and visually stable regardless of song theme.
    let settings_font = ttf_ctx
        .load_font("assets/fonts/SyneMono-Regular.ttf", 25)
        .map_err(|e| format!("Failed to load settings sans-serif font: {e}"))?;

    let mut loaded_font_theme = selected_theme;

    let scene_w = preset.width;
    let scene_h = preset.height;
    let layout = scene_layout(preset);

    let mut event_pump = sdl.event_pump()?;
    let mut runtime_artwork_mode = ctx.config.artwork.mode.clone();
    let mut runtime_visualizer_mode = ctx.config.visualizer.mode.clone();
    let mut runtime_spectrum = RuntimeSpectrumSettings::from_config(&ctx);
    let mut runtime_visualizer_gain = ctx.config.visualizer.gain;
    let mut runtime_display_orientation = ctx.config.display.orientation.clone();
    let mut runtime_display_rotation = configured_rotation.canonical().to_string();
    if DisplayRotation::parse(&ctx.config.display.rotation).is_none() {
        log_error(
            &ctx,
            &format!(
                "Invalid display.rotation '{}'; using 'normal'",
                ctx.config.display.rotation
            ),
        );
    }
    let mut settings_open = false;
    let mut settings_selected = 0usize;
    let mut settings_original_artwork = runtime_artwork_mode.clone();
    let mut settings_original_visualizer = runtime_visualizer_mode.clone();
    let mut settings_original_spectrum = runtime_spectrum.clone();
    let mut settings_original_gain = runtime_visualizer_gain;
    let mut settings_original_orientation = runtime_display_orientation.clone();
    let mut settings_original_rotation = runtime_display_rotation.clone();
    let mut settings_status = String::new();
    let mut loaded_version: u64 = u64::MAX;
    let mut loaded_track_identity = String::new();
    let mut artwork_texture: Option<Texture<'_>> = None;
    let mut previous_artwork_texture: Option<Texture<'_>> = None;
    let mut circular_artwork_texture: Option<Texture<'_>> = None;
    let mut previous_circular_artwork_texture: Option<Texture<'_>> = None;
    let mut artwork_started_at = Instant::now();
    let mut visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
    let mut last_canvas_size: Option<(u32, u32)> = None;
    let mut display_peak = 0.0f32;
    let mut vu_display_level = 0.0f32;
    let mut last_vis_debug = Instant::now();
    let mut last_frame_log = Instant::now();
    let mut frame_counter: u32 = 0;
    let mut frame_timer = Instant::now();
    let mut text_scroll_started_at = Instant::now();

    let mut smoothed_upper_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut smoothed_lower_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut upper_peak_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut lower_peak_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut last_spectrum_peak_drop = Instant::now();

    let mut static_scene_cache: Option<StaticSceneCache<'_>> = None;
    let mut static_scene_texture = texture_creator
        .create_texture_target(PixelFormatEnum::RGBA8888, scene_w, scene_h)
        .map_err(|e| e.to_string())?;
    let mut composed_frame_texture = texture_creator
        .create_texture_target(PixelFormatEnum::RGBA8888, scene_w, scene_h)
        .map_err(|e| e.to_string())?;

    let vu_face_texture = match texture_creator.load_texture("assets/vu/vintage-meter-face-v2.png")
    {
        Ok(texture) => Some(texture),
        Err(e) => {
            log_error(
                &ctx,
                &format!("Failed to load vintage VU meter face; using fallback: {e}"),
            );
            None
        }
    };

    // The detailed groove geometry is expensive to redraw every frame on a
    // Raspberry Pi. Render it once, then rotate the complete vinyl surface as
    // a cached texture together with the album label.
    let record_scene = compute_record_rect(layout.artwork_region);
    let mut vinyl_texture = {
        let diameter = record_scene.width().max(1);
        let mut texture = texture_creator
            .create_texture_target(PixelFormatEnum::RGBA8888, diameter, diameter)
            .map_err(|e| e.to_string())?;
        texture.set_blend_mode(BlendMode::Blend);
        canvas
            .with_texture_canvas(&mut texture, |vinyl_canvas| {
                vinyl_canvas.set_draw_color(Color::RGBA(0, 0, 0, 0));
                vinyl_canvas.clear();
                let _ = draw_vinyl_record(vinyl_canvas, Rect::new(0, 0, diameter, diameter), 1.0);
            })
            .map_err(|e| e.to_string())?;
        Some(texture)
    };

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => running.store(false, Ordering::SeqCst),
                Event::KeyDown {
                    keycode: Some(key),
                    repeat: false,
                    ..
                } => {
                    if settings_open {
                        match key {
                            Keycode::Escape => {
                                runtime_artwork_mode = settings_original_artwork.clone();
                                runtime_visualizer_mode = settings_original_visualizer.clone();
                                runtime_spectrum = settings_original_spectrum.clone();
                                runtime_visualizer_gain = settings_original_gain;
                                runtime_display_orientation = settings_original_orientation.clone();
                                runtime_display_rotation = settings_original_rotation.clone();
                                settings_status.clear();
                                settings_open = false;
                            }
                            Keycode::Up => {
                                settings_selected = settings_selected.saturating_sub(1);
                                settings_status.clear();
                            }
                            Keycode::Down => {
                                let row_count =
                                    settings_rows(&runtime_visualizer_mode, &runtime_spectrum)
                                        .len();
                                settings_selected =
                                    (settings_selected + 1).min(row_count.saturating_sub(1));
                                settings_status.clear();
                            }
                            Keycode::Left | Keycode::Right => {
                                let direction = if key == Keycode::Right { 1 } else { -1 };
                                let rows =
                                    settings_rows(&runtime_visualizer_mode, &runtime_spectrum);
                                let selected_row = rows
                                    .get(settings_selected)
                                    .copied()
                                    .unwrap_or(SettingsRow::Artwork);

                                match selected_row {
                                    SettingsRow::Artwork => {
                                        runtime_artwork_mode = cycle_option(
                                            &runtime_artwork_mode,
                                            &["cover", "turntable"],
                                            direction,
                                        );
                                        artwork_started_at = Instant::now();
                                    }
                                    SettingsRow::Visualizer => {
                                        runtime_visualizer_mode = cycle_option(
                                            &runtime_visualizer_mode,
                                            &["spectrum", "oscilloscope", "analog_vu"],
                                            direction,
                                        );
                                    }
                                    SettingsRow::SpectrumStyle => {
                                        runtime_spectrum.render_style = cycle_option(
                                            &runtime_spectrum.render_style,
                                            &["full", "top_only", "segmented"],
                                            direction,
                                        );
                                    }
                                    SettingsRow::SegmentRows => {
                                        runtime_spectrum.segment_rows = ((runtime_spectrum
                                            .segment_rows
                                            as i32)
                                            + direction * 2)
                                            .clamp(4, 96)
                                            as u32;
                                    }
                                    SettingsRow::SegmentHeight => {
                                        runtime_spectrum.segment_height =
                                            ((runtime_spectrum.segment_height as i32) + direction)
                                                .clamp(1, 24)
                                                as u32;
                                    }
                                    SettingsRow::SegmentGap => {
                                        runtime_spectrum.segment_gap =
                                            ((runtime_spectrum.segment_gap as i32) + direction)
                                                .clamp(0, 24)
                                                as u32;
                                    }
                                    SettingsRow::SegmentColumnGap => {
                                        runtime_spectrum.segment_column_gap =
                                            ((runtime_spectrum.segment_column_gap as i32)
                                                + direction)
                                                .clamp(0, 80)
                                                as u32;
                                    }
                                    SettingsRow::SegmentInactive => {
                                        runtime_spectrum.segment_inactive =
                                            !runtime_spectrum.segment_inactive;
                                    }
                                    SettingsRow::Sensitivity => {
                                        runtime_visualizer_gain = (runtime_visualizer_gain
                                            + direction as f32 * 0.25)
                                            .clamp(0.25, 8.0);
                                    }
                                    SettingsRow::Orientation => {
                                        runtime_display_orientation = cycle_option(
                                            &runtime_display_orientation,
                                            &["portrait", "landscape"],
                                            direction,
                                        );
                                    }
                                    SettingsRow::Rotation => {
                                        runtime_display_rotation = cycle_option(
                                            &runtime_display_rotation,
                                            &[
                                                "normal",
                                                "clockwise",
                                                "inverted",
                                                "counter_clockwise",
                                            ],
                                            direction,
                                        );
                                    }
                                }

                                let row_count =
                                    settings_rows(&runtime_visualizer_mode, &runtime_spectrum)
                                        .len();
                                settings_selected =
                                    settings_selected.min(row_count.saturating_sub(1));

                                settings_status = if matches!(
                                    selected_row,
                                    SettingsRow::Orientation | SettingsRow::Rotation
                                ) {
                                    "Save + restart".to_string()
                                } else {
                                    "Preview".to_string()
                                };
                            }
                            Keycode::S => match save_display_modes(
                                "config/songart.toml",
                                &runtime_artwork_mode,
                                &runtime_visualizer_mode,
                                &runtime_spectrum,
                                runtime_visualizer_gain,
                                &runtime_display_orientation,
                                &runtime_display_rotation,
                            ) {
                                Ok(()) => {
                                    settings_original_artwork = runtime_artwork_mode.clone();
                                    settings_original_visualizer = runtime_visualizer_mode.clone();
                                    settings_original_spectrum = runtime_spectrum.clone();
                                    settings_original_gain = runtime_visualizer_gain;
                                    settings_original_orientation =
                                        runtime_display_orientation.clone();
                                    settings_original_rotation = runtime_display_rotation.clone();
                                    settings_status = "Saved".to_string();
                                }
                                Err(e) => {
                                    settings_status = "Save failed".to_string();
                                    log_error(&ctx, &e);
                                }
                            },
                            Keycode::Return | Keycode::KpEnter => {
                                settings_status.clear();
                                settings_open = false;
                            }
                            Keycode::M | Keycode::F1 => {
                                settings_status.clear();
                                settings_open = false;
                            }
                            _ => {}
                        }
                    } else {
                        match key {
                            Keycode::M | Keycode::F1 => {
                                settings_original_artwork = runtime_artwork_mode.clone();
                                settings_original_visualizer = runtime_visualizer_mode.clone();
                                settings_original_spectrum = runtime_spectrum.clone();
                                settings_original_gain = runtime_visualizer_gain;
                                settings_original_orientation = runtime_display_orientation.clone();
                                settings_original_rotation = runtime_display_rotation.clone();
                                settings_selected = 0;
                                settings_status.clear();
                                settings_open = true;
                            }
                            Keycode::Escape => running.store(false, Ordering::SeqCst),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        let mut state = {
            let state_guard = shared_state.lock().unwrap();
            state_guard.clone()
        };

        let (
            audio_len,
            sample_len,
            live_level,
            left_points,
            right_points,
            raw_upper_bins,
            raw_lower_bins,
        ) = {
            let audio = shared_audio.lock().unwrap();
            let vis_samples = audio.recent_ms(ctx.config.visualizer.window_ms);

            let level = compute_rms(&vis_samples).unwrap_or(0.0);

            let left = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.left_y_offset,
                ctx.config.visualizer.y_scale,
                runtime_visualizer_gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain,
            );

            let right = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.right_y_offset,
                ctx.config.visualizer.y_scale,
                runtime_visualizer_gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain,
            );

            let bins = compute_spectrum_bins(
                &vis_samples,
                ctx.config.audio.sample_rate,
                ctx.config.visualizer.spectrum_fft_size,
                ctx.config.visualizer.spectrum_bin_count,
                ctx.config.visualizer.spectrum_min_hz,
                ctx.config.visualizer.spectrum_max_hz,
                runtime_visualizer_gain,
                ctx.config.visualizer.max_gain,
                ctx.config.visualizer.spectrum_log_epsilon,
                ctx.config.visualizer.spectrum_log_scale,
                ctx.config.visualizer.spectrum_log_offset,
                ctx.config.visualizer.spectrum_noise_floor,
                ctx.config.visualizer.spectrum_contrast,
            );

            (
                audio.len(),
                vis_samples.len(),
                level,
                left,
                right,
                bins.clone(),
                bins,
            )
        };

        // Faster rise, slower fall makes the spectrum feel lively without looking jittery.
        let rise = ctx.config.visualizer.spectrum_attack.clamp(0.0, 1.0);
        let fall = ctx.config.visualizer.spectrum_smoothing.clamp(0.0, 0.98);

        update_smoothed_bins(&mut smoothed_upper_bins, &raw_upper_bins, rise, fall);
        update_smoothed_bins(&mut smoothed_lower_bins, &raw_lower_bins, rise, fall);

        if ctx.config.visualizer.peaks.enabled {
            let hold = Duration::from_millis(ctx.config.visualizer.peaks.hold_ms);
            let should_drop = last_spectrum_peak_drop.elapsed() >= hold;
            let top_only = runtime_spectrum.top_only();
            let peak_scale = if top_only {
                ctx.config.visualizer.height.max(1)
            } else {
                (ctx.config.visualizer.height / 2).max(1)
            } as f32;
            let drop_amount = (ctx.config.visualizer.peaks.drop_pixels as f32) / peak_scale;

            let upper_rose = update_spectrum_peaks(
                &mut upper_peak_bins,
                &smoothed_upper_bins,
                drop_amount,
                should_drop,
            );
            let lower_rose = update_spectrum_peaks(
                &mut lower_peak_bins,
                &smoothed_lower_bins,
                drop_amount,
                should_drop,
            );

            if upper_rose || lower_rose || should_drop {
                last_spectrum_peak_drop = Instant::now();
            }
        } else {
            upper_peak_bins.fill(0.0);
            lower_peak_bins.fill(0.0);
            last_spectrum_peak_drop = Instant::now();
        }

        display_peak = if live_level > display_peak {
            live_level
        } else {
            display_peak * 0.96
        };

        let vu_target = (live_level * runtime_visualizer_gain).clamp(0.0, 1.4);
        let vu_response = if vu_target > vu_display_level {
            0.28
        } else {
            0.055
        };
        vu_display_level += (vu_target - vu_display_level) * vu_response;

        state.meter.level = vu_display_level;
        state.meter.peak = if ctx.config.visualizer.peak_hold {
            display_peak
        } else {
            live_level
        };

        state.visualizer.enabled = ctx.config.visualizer.enabled;
        state.visualizer.mode = visualizer_mode_from_config(&runtime_visualizer_mode);
        state.visualizer.frame.left_points = left_points;
        state.visualizer.frame.right_points = right_points;

        if last_vis_debug.elapsed()
            >= Duration::from_millis(ctx.config.visualizer.debug_log_interval_ms)
        {
            log_visualizer_debug(
                &ctx,
                audio_len,
                sample_len,
                live_level,
                &smoothed_upper_bins,
            );
            last_vis_debug = Instant::now();
        }

        let track_identity = format!("{}\0{}", state.title, state.artist);
        if state.version != loaded_version && track_identity == loaded_track_identity {
            // Metadata-only updates for the current song should rebuild text,
            // but must not restart its artwork animation.
            loaded_version = state.version;
        } else if state.version != loaded_version {
            if !state.artwork_path.is_empty() && Path::new(&state.artwork_path).exists() {
                match texture_creator.load_texture(&state.artwork_path) {
                    Ok(mut texture) => {
                        // Loaded JPG/PNG textures may default to BlendMode::None,
                        // in which case alpha modulation looks like a hard cut.
                        texture.set_blend_mode(BlendMode::Blend);
                        previous_artwork_texture = artwork_texture.take();
                        previous_circular_artwork_texture = circular_artwork_texture.take();
                        artwork_texture = Some(texture);
                        let record_rect = compute_record_rect(layout.artwork_region);
                        let circular_diameter = record_rect.width().max(1);
                        let mut circular_texture = texture_creator
                            .create_texture_target(
                                PixelFormatEnum::RGBA8888,
                                circular_diameter,
                                circular_diameter,
                            )
                            .map_err(|e| e.to_string())?;
                        circular_texture.set_blend_mode(BlendMode::Blend);
                        canvas
                            .with_texture_canvas(&mut circular_texture, |label_canvas| {
                                label_canvas.set_draw_color(Color::RGBA(0, 0, 0, 0));
                                label_canvas.clear();
                                if let Some(source) = artwork_texture.as_ref() {
                                    let _ = draw_circular_artwork(
                                        label_canvas,
                                        source,
                                        Rect::new(0, 0, circular_diameter, circular_diameter),
                                    );
                                }
                            })
                            .map_err(|e| e.to_string())?;
                        circular_artwork_texture = Some(circular_texture);
                        artwork_started_at = Instant::now();
                        visualizer_colors =
                            visualizer_colors_for_artwork(&ctx, Some(&state.artwork_path));
                        loaded_version = state.version;
                        loaded_track_identity = track_identity.clone();
                        log_debug(
                            &ctx,
                            &format!(
                                "Renderer loaded artwork version {} from {}",
                                loaded_version, state.artwork_path
                            ),
                        );
                    }
                    Err(e) => {
                        log_error(&ctx, &format!("Renderer failed to load artwork: {e}"));
                        artwork_texture = None;
                        previous_artwork_texture = None;
                        circular_artwork_texture = None;
                        previous_circular_artwork_texture = None;
                        visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                    }
                }
            } else {
                artwork_texture = None;
                previous_artwork_texture = None;
                circular_artwork_texture = None;
                previous_circular_artwork_texture = None;
                visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                loaded_version = state.version;
                loaded_track_identity = track_identity;
            }
        }

        let (canvas_w, canvas_h) = canvas.output_size().map_err(|e| e.to_string())?;
        if last_canvas_size != Some((canvas_w, canvas_h)) {
            log_debug(
                &ctx,
                &format!("Canvas output size: {}x{}", canvas_w, canvas_h),
            );
            last_canvas_size = Some((canvas_w, canvas_h));
        }

        let needs_static_rebuild = static_scene_cache
            .as_ref()
            .map(|c| c.version != state.version)
            .unwrap_or(true);

        if needs_static_rebuild {
            let (title_font_path, body_font_path, title_font_size, body_font_size, selected_theme) =
                selected_fonts(&ctx, &state);

            log_debug(
                &ctx,
                &format!(
                    "Font theme evaluation: mode='{}' genre='{}' released='{}' selected='{}' loaded='{}'",
                    ctx.config.fonts.mode,
                    state.genre,
                    state.released,
                    selected_theme,
                    loaded_font_theme
                ),
            );

            if selected_theme != loaded_font_theme {
                log_info(
                    &ctx,
                    &format!(
                        "Changing font theme from '{}' to '{}' for genre='{}' released='{}' title_font='{}' body_font='{}' title_size={} body_size={}",
                        loaded_font_theme,
                        selected_theme,
                        state.genre,
                        state.released,
                        title_font_path,
                        body_font_path,
                        title_font_size,
                        body_font_size
                    ),
                );

                title_font = ttf_ctx
                    .load_font(title_font_path, title_font_size)
                    .map_err(|e| {
                        format!("Failed to load title font from {}: {e}", title_font_path)
                    })?;

                body_font = ttf_ctx
                    .load_font(body_font_path, body_font_size)
                    .map_err(|e| {
                        format!("Failed to load body font from {}: {e}", body_font_path)
                    })?;

                loaded_font_theme = selected_theme;
            }

            let text = build_text_cache(
                &texture_creator,
                &title_font,
                &body_font,
                &state,
                preset,
                &layout,
            )?;

            let artwork_rect = artwork_texture
                .as_ref()
                .map(|texture| compute_artwork_rect(texture.query(), layout.artwork_region));

            let cache = StaticSceneCache {
                version: state.version,
                text,
                artwork_rect,
            };

            canvas
                .with_texture_canvas(&mut static_scene_texture, |tex_canvas| {
                    let _ = cache.draw_static_scene(tex_canvas, &ctx, &layout);
                })
                .map_err(|e| e.to_string())?;

            static_scene_cache = Some(cache);
            text_scroll_started_at = Instant::now();
            log_debug(
                &ctx,
                &format!("Rebuilt static scene for version {}", state.version),
            );
        }

        let mut frame_draw_result = Ok(());
        canvas
            .with_texture_canvas(&mut composed_frame_texture, |mut canvas| {
                frame_draw_result = (|| -> Result<(), String> {
        let canvas_w = scene_w;
        let canvas_h = scene_h;
        let scale_x = (canvas_w as f32) / (scene_w as f32);
        let scale_y = (canvas_h as f32) / (scene_h as f32);
        let scene_scale = f32::min(scale_x, scale_y);

        let render_w = ((scene_w as f32) * scene_scale) as u32;
        let render_h = ((scene_h as f32) * scene_scale) as u32;

        let offset_x = ((canvas_w - render_w) / 2) as i32;
        let offset_y = ((canvas_h - render_h) / 2) as i32;

        let sx = |x: i32| offset_x + (((x as f32) * scene_scale) as i32);
        let sy = |y: i32| offset_y + (((y as f32) * scene_scale) as i32);
        let sw = |w: u32| ((w as f32) * scene_scale) as u32;
        let sh = |h: u32| ((h as f32) * scene_scale) as u32;

        canvas.set_draw_color(canvas_background_color(&ctx));
        canvas.clear();

        let static_target = Rect::new(offset_x, offset_y, render_w, render_h);
        canvas.copy(&static_scene_texture, None, static_target)?;

        const ARTWORK_FADE_SECONDS: f32 = 1.5;
        let artwork_elapsed = artwork_started_at.elapsed().as_secs_f32();
        if artwork_elapsed >= ARTWORK_FADE_SECONDS {
            previous_artwork_texture = None;
            previous_circular_artwork_texture = None;
        }

        if runtime_artwork_mode.eq_ignore_ascii_case("turntable") {
            if let (Some(artwork), Some(cache)) =
                (artwork_texture.as_mut(), static_scene_cache.as_ref())
            {
                if let Some(cover_scene) = cache.artwork_rect {
                    let elapsed = artwork_elapsed;
                    let cover = Rect::new(
                        sx(cover_scene.x()),
                        sy(cover_scene.y()),
                        sw(cover_scene.width()),
                        sh(cover_scene.height()),
                    );

                    if elapsed < ARTWORK_FADE_SECONDS {
                        let fade = (elapsed / ARTWORK_FADE_SECONDS).clamp(0.0, 1.0);
                        let record = Rect::new(
                            sx(record_scene.x()),
                            sy(record_scene.y()),
                            sw(record_scene.width()),
                            sh(record_scene.height()),
                        );
                        if let Some(previous_label) =
                            previous_circular_artwork_texture.as_mut()
                        {
                            let label_diameter = record.width() / 3;
                            let label = Rect::new(
                                record.x() + (record.width() - label_diameter) as i32 / 2,
                                record.y() + (record.height() - label_diameter) as i32 / 2,
                                label_diameter,
                                label_diameter,
                            );
                            let rotation = (elapsed as f64 * 200.0) % 360.0;
                            if let Some(vinyl) = vinyl_texture.as_mut() {
                                vinyl.set_alpha_mod(((1.0 - fade) * 255.0).round() as u8);
                                canvas.copy_ex(
                                    vinyl,
                                    None,
                                    record,
                                    rotation,
                                    None,
                                    false,
                                    false,
                                )?;
                                vinyl.set_alpha_mod(255);
                            }
                            previous_label
                                .set_alpha_mod(((1.0 - fade) * 255.0).round() as u8);
                            canvas.copy_ex(
                                previous_label,
                                None,
                                label,
                                rotation,
                                None,
                                false,
                                false,
                            )?;
                            previous_label.set_alpha_mod(255);
                        }
                        artwork.set_alpha_mod((fade * 255.0).round() as u8);
                        canvas.copy(artwork, None, cover)?;
                        artwork.set_alpha_mod(255);
                    } else if elapsed < ARTWORK_FADE_SECONDS + 5.0 {
                        canvas.copy(artwork, None, cover)?;
                    } else {
                        let record = Rect::new(
                            sx(record_scene.x()),
                            sy(record_scene.y()),
                            sw(record_scene.width()),
                            sh(record_scene.height()),
                        );

                        let label_diameter = record.width() / 3;
                        let label = Rect::new(
                            record.x() + (record.width() - label_diameter) as i32 / 2,
                            record.y() + (record.height() - label_diameter) as i32 / 2,
                            label_diameter,
                            label_diameter,
                        );

                        const CROP_SECONDS: f32 = 2.0;
                        const SHRINK_SECONDS: f32 = 2.5;
                        let morph_elapsed = elapsed - ARTWORK_FADE_SECONDS - 5.0;
                        if morph_elapsed < CROP_SECONDS {
                            let linear_crop = (morph_elapsed / CROP_SECONDS).clamp(0.0, 1.0);
                            let crop =
                                linear_crop * linear_crop * (3.0 - 2.0 * linear_crop);
                            artwork.set_alpha_mod(((1.0 - crop) * 255.0).round() as u8);
                            canvas.copy(artwork, None, cover)?;
                            artwork.set_alpha_mod(255);

                            if let Some(circular) = circular_artwork_texture.as_mut() {
                                circular.set_alpha_mod((crop * 255.0).round() as u8);
                                canvas.copy(circular, None, record)?;
                                circular.set_alpha_mod(255);
                            }
                        } else {
                            let shrink_elapsed = morph_elapsed - CROP_SECONDS;
                            let linear_progress =
                                (shrink_elapsed / SHRINK_SECONDS).clamp(0.0, 1.0);
                            let progress =
                                linear_progress * linear_progress * (3.0 - 2.0 * linear_progress);
                            let interpolate = |start: i32, end: i32| {
                                (start as f32 + (end - start) as f32 * progress).round() as i32
                            };
                            let interpolate_size = |start: u32, end: u32| {
                                (start as f32 + (end as f32 - start as f32) * progress).round()
                                    as u32
                            };
                            let shrinking_disc = Rect::new(
                                interpolate(record.x(), label.x()),
                                interpolate(record.y(), label.y()),
                                interpolate_size(record.width(), label.width()),
                                interpolate_size(record.height(), label.height()),
                            );

                            // 33 1/3 RPM equals 200 degrees per second.
                            let rotation = (shrink_elapsed as f64 * 200.0) % 360.0;
                            if let Some(vinyl) = vinyl_texture.as_ref() {
                                canvas.copy_ex(
                                    vinyl,
                                    None,
                                    record,
                                    rotation,
                                    None,
                                    false,
                                    false,
                                )?;
                            }
                            if let Some(circular) = circular_artwork_texture.as_ref() {
                                canvas.copy_ex(
                                    circular,
                                    None,
                                    shrinking_disc,
                                    rotation,
                                    None,
                                    false,
                                    false,
                                )?;
                            }

                            let center_x = record.x() + record.width() as i32 / 2;
                            let center_y = record.y() + record.height() as i32 / 2;
                            let spindle_radius = (record.width() / 160).max(2) as i32;
                            draw_filled_circle(
                                &mut canvas,
                                center_x,
                                center_y,
                                spindle_radius,
                                Color::RGB(210, 210, 205),
                            )?;
                        }
                    }
                }
            }
        } else if let (Some(artwork), Some(cache)) =
            (artwork_texture.as_mut(), static_scene_cache.as_ref())
        {
            if let Some(cover_scene) = cache.artwork_rect {
                let cover = Rect::new(
                    sx(cover_scene.x()),
                    sy(cover_scene.y()),
                    sw(cover_scene.width()),
                    sh(cover_scene.height()),
                );
                let fade = (artwork_elapsed / ARTWORK_FADE_SECONDS).clamp(0.0, 1.0);
                if let Some(previous) = previous_artwork_texture.as_mut() {
                    let previous_scene =
                        compute_artwork_rect(previous.query(), layout.artwork_region);
                    let previous_target = Rect::new(
                        sx(previous_scene.x()),
                        sy(previous_scene.y()),
                        sw(previous_scene.width()),
                        sh(previous_scene.height()),
                    );
                    previous.set_alpha_mod(((1.0 - fade) * 255.0).round() as u8);
                    canvas.copy(previous, None, previous_target)?;
                    previous.set_alpha_mod(255);
                }
                artwork.set_alpha_mod((fade * 255.0).round() as u8);
                canvas.copy(artwork, None, cover)?;
                artwork.set_alpha_mod(255);
            }
        }

        if let Some(cache) = static_scene_cache.as_ref() {
            cache.draw_text(
                &mut canvas,
                offset_x,
                offset_y,
                scene_scale,
                text_scroll_started_at.elapsed(),
            )?;
        }

        if ctx.config.visualizer.enabled && state.visualizer.enabled {
            let padding = ctx.config.visualizer.padding;
            let vis_h = ctx
                .config
                .visualizer
                .height
                .min(layout.visualizer_region.height().saturating_sub(padding * 2));
            let vis_x_scene = layout.visualizer_region.x() + padding as i32;
            let vis_y_scene = layout.visualizer_region.y()
                + layout.visualizer_region.height() as i32
                - vis_h as i32
                - padding as i32;
            let vis_w_scene = layout
                .visualizer_region
                .width()
                .saturating_sub(padding * 2);

            draw_visualizer(
                &mut canvas,
                &ctx,
                &visualizer_colors,
                state.visualizer.mode,
                &state.visualizer.frame.left_points,
                &state.visualizer.frame.right_points,
                &smoothed_upper_bins,
                &smoothed_lower_bins,
                &upper_peak_bins,
                &lower_peak_bins,
                sx(vis_x_scene),
                sy(vis_y_scene),
                sw(vis_w_scene),
                sh(vis_h),
                ctx.config.visualizer.spectrum_bar_gap,
                state.meter.level,
                vu_face_texture.as_ref(),
                &runtime_spectrum,
            )?;
        }

        if settings_open {
            draw_settings_overlay(
                &mut canvas,
                &texture_creator,
                &settings_font,
                &runtime_artwork_mode,
                &runtime_visualizer_mode,
                &runtime_spectrum,
                runtime_visualizer_gain,
                &runtime_display_orientation,
                &runtime_display_rotation,
                settings_selected,
                &settings_status,
            )?;
        }

        Ok(())
                })();
            })
            .map_err(|e| e.to_string())?;
        frame_draw_result?;

        let output_rotation =
            DisplayRotation::parse(&runtime_display_rotation).unwrap_or(DisplayRotation::Normal);
        let angle = output_rotation.angle();
        let rotated_w = if output_rotation.swaps_dimensions() {
            scene_h
        } else {
            scene_w
        };
        let rotated_h = if output_rotation.swaps_dimensions() {
            scene_w
        } else {
            scene_h
        };
        let output_scale = f32::min(
            canvas_w as f32 / rotated_w as f32,
            canvas_h as f32 / rotated_h as f32,
        );
        let destination_w = ((scene_w as f32) * output_scale).max(1.0) as u32;
        let destination_h = ((scene_h as f32) * output_scale).max(1.0) as u32;
        let destination = Rect::new(
            canvas_w as i32 / 2 - destination_w as i32 / 2,
            canvas_h as i32 / 2 - destination_h as i32 / 2,
            destination_w,
            destination_h,
        );

        canvas.set_draw_color(canvas_background_color(&ctx));
        canvas.clear();
        canvas.copy_ex(
            &composed_frame_texture,
            None,
            destination,
            angle,
            None,
            false,
            false,
        )?;
        canvas.present();

        frame_counter += 1;
        if last_frame_log.elapsed() >= Duration::from_secs(5) {
            let avg_ms = (frame_timer.elapsed().as_secs_f64() * 1000.0) / (frame_counter as f64);
            log_info(&ctx, &format!("Display avg frame time: {:.1} ms", avg_ms));
            last_frame_log = Instant::now();
            frame_timer = Instant::now();
            frame_counter = 0;
        }

        thread::sleep(Duration::from_millis(ctx.config.display.frame_delay_ms));
    }

    log_info(&ctx, "Display loop stopped.");
    Ok(())
}
