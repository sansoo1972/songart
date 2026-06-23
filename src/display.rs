use crate::audio::{build_oscilloscope_points, compute_rms, SharedAudioBuffer};
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
use sdl2::render::{Texture, TextureCreator, TextureQuery};
use sdl2::surface::Surface;
use sdl2::video::WindowContext;

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

// ==============================================================================
// Text + Metadata Helpers
// ==============================================================================

fn album_or_release_line(state: &SongState) -> String {
    let mut third_line = state.album.clone();

    if !state.released.is_empty() && state.released != "Unknown" {
        if third_line.is_empty() || third_line == "Unknown" {
            third_line = state.released.clone();
        } else {
            third_line = format!("{} • {}", state.album, state.released);
        }
    }

    if third_line.trim().is_empty() {
        "Album unknown".to_string()
    } else {
        third_line
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

    if max <= 0.0 {
        0.0
    } else {
        (max - min) / max
    }
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

    if hue < 0.0 {
        hue + 360.0
    } else {
        hue
    }
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

fn metadata_font_theme(ctx: &AppContext, state: &SongState) -> String {
    let genre = state.genre.to_ascii_lowercase();
    let year = parse_release_year(&state.released);

    // Strong era/vibe override for synth-heavy 80s music.
    if contains_any(
        &genre,
        &["electronic", "synth", "synth-pop", "new wave", "dance"],
    ) || matches!(year, Some(1980..=1989))
    {
        return "techy".to_string();
    }

    // 90s rock/alternative/grunge gets a rougher style.
    if contains_any(&genre, &["rock", "alternative", "grunge", "punk"])
        && matches!(year, Some(1990..=1999))
    {
        return "grungy".to_string();
    }

    // Older music gets a retro display treatment.
    if matches!(year, Some(0..=1979)) {
        return "retro".to_string();
    }

    // Soundtracks, scores, classical, and orchestral music.
    if contains_any(&genre, &["classical", "soundtrack", "score", "orchestral"]) {
        return "fantasy".to_string();
    }

    // Acoustic / folk / singer-songwriter / country / latin.
    if contains_any(
        &genre,
        &["folk", "acoustic", "country", "singer-songwriter", "latin"],
    ) {
        return "scripted".to_string();
    }

    // Modern mainstream genres.
    if contains_any(&genre, &["pop", "r&b", "hip-hop", "rap"]) || matches!(year, Some(2000..=9999))
    {
        return "modern".to_string();
    }

    ctx.config.fonts.fallback_theme.to_ascii_lowercase()
}

fn selected_font_theme(ctx: &AppContext, state: &SongState) -> String {
    match ctx.config.fonts.mode.to_ascii_lowercase().as_str() {
        "metadata" => metadata_font_theme(ctx, state),
        _ => ctx.config.fonts.theme.to_ascii_lowercase(),
    }
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

    fn scroll_offset(&self, elapsed: Duration) -> u32 {
        const START_PAUSE_SECS: f32 = 5.0;
        const SCROLL_PIXELS_PER_SEC: f32 = 55.0;

        if self.rect.width() <= self.viewport_width {
            return 0;
        }

        let overflow = self.rect.width() - self.viewport_width;
        let scroll_secs = (overflow as f32) / SCROLL_PIXELS_PER_SEC;
        let cycle_secs = START_PAUSE_SECS + scroll_secs;
        let cycle_pos = elapsed.as_secs_f32() % cycle_secs;

        if cycle_pos < START_PAUSE_SECS {
            0
        } else {
            ((cycle_pos - START_PAUSE_SECS) * SCROLL_PIXELS_PER_SEC)
                .round()
                .clamp(0.0, overflow as f32) as u32
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
        let src_x = self.scroll_offset(elapsed);
        let visible_w = self
            .viewport_width
            .min(self.rect.width().saturating_sub(src_x))
            .max(1);

        let src = Rect::new(src_x as i32, 0, visible_w, self.rect.height());
        let dst = Rect::new(
            offset_x + (((self.rect.x()) as f32 * scale) as i32),
            offset_y + (((self.rect.y()) as f32 * scale) as i32),
            ((visible_w as f32) * scale).max(1.0) as u32,
            ((self.rect.height() as f32) * scale).max(1.0) as u32,
        );

        let clip = Rect::new(
            dst.x(),
            dst.y(),
            ((self.viewport_width as f32) * scale).max(1.0) as u32,
            dst.height(),
        );

        canvas.set_clip_rect(Some(clip));
        let result = canvas.copy(&self.texture, src, dst);
        canvas.set_clip_rect(None);

        result?;
        Ok(())
    }
}

struct TextCache<'a> {
    title: CachedText<'a>,
    artist: CachedText<'a>,
    third: CachedText<'a>,
    detail: CachedText<'a>,
}

fn build_text_cache<'a>(
    texture_creator: &'a TextureCreator<WindowContext>,
    title_font: &sdl2::ttf::Font,
    body_font: &sdl2::ttf::Font,
    state: &SongState,
    preset: &DisplayPreset,
    top_h: u32,
) -> Result<TextCache<'a>, String> {
    let panel_x = preset.panel_x;
    let mut panel_y = (top_h as i32) + preset.panel_y;
    let viewport_width = preset
        .width
        .saturating_sub((panel_x as u32).saturating_mul(2));

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

    let third_line = album_or_release_line(state);

    let detail_line = format!(
        "Genre: {}    Composer: {}",
        state.genre,
        state.composer
    );

    let title = CachedText::new(
        texture_creator,
        title_font,
        &title_line,
        Color::RGB(255, 255, 255),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.title_line_spacing;

    let artist = CachedText::new(
        texture_creator,
        body_font,
        &artist_line,
        Color::RGB(210, 210, 210),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.body_line_spacing;

    let third = CachedText::new(
        texture_creator,
        body_font,
        &third_line,
        Color::RGB(180, 180, 180),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.detail_line_spacing;

    let detail = CachedText::new(
        texture_creator,
        body_font,
        &detail_line,
        Color::RGB(140, 140, 140),
        panel_x,
        panel_y,
        viewport_width,
    )?;

    Ok(TextCache {
        title,
        artist,
        third,
        detail,
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

    canvas.set_draw_color(Color::RGB(40, 40, 40));
    canvas.draw_line(
        Point::new(x, y + (height as i32) / 4),
        Point::new(x + (width as i32), y + (height as i32) / 4),
    )?;
    canvas.draw_line(
        Point::new(x, y + ((height as i32) * 3) / 4),
        Point::new(x + (width as i32), y + ((height as i32) * 3) / 4),
    )?;

    draw_polyline(canvas, left_points, x, y, width, height, colors.upper)?;

    draw_polyline(canvas, right_points, x, y, width, height, colors.lower)?;

    Ok(())
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

    let total_gap = bar_gap.saturating_mul(count.saturating_sub(1));
    let bar_w = (width.saturating_sub(total_gap) / count).max(1);
    let top_only = ctx
        .config
        .visualizer
        .spectrum
        .render_style
        .eq_ignore_ascii_case("top_only");
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
            &ctx.config.visualizer.spectrum.render_style,
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
                &ctx.config.visualizer.spectrum.render_style,
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
) -> Result<(), String> {
    match mode {
        VisualizerMode::None => Ok(()),
        VisualizerMode::Oscilloscope | VisualizerMode::AnalogVu => draw_oscilloscope(
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
        ),
    }
}

// ==============================================================================
// Static Scene Drawing
// ==============================================================================

fn compute_artwork_rect(query: TextureQuery, scene_w: u32, top_h: u32) -> Rect {
    let art_w = query.width as f32;
    let art_h = query.height as f32;

    let padding = 24.0;
    let max_w = (scene_w as f32) - padding * 2.0;
    let max_h = (top_h as f32) - padding * 2.0;

    let scale = f32::min(max_w / art_w, max_h / art_h);
    let draw_w = (art_w * scale) as u32;
    let draw_h = (art_h * scale) as u32;

    let x = ((scene_w - draw_w) / 2) as i32;
    let y = ((top_h - draw_h) / 2) as i32;

    Rect::new(x, y, draw_w, draw_h)
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
        artwork_texture: Option<&Texture<'a>>,
        scene_w: u32,
        top_h: u32,
        bottom_h: u32,
    ) -> Result<(), String> {
        canvas.set_draw_color(canvas_background_color(ctx));
        canvas.clear();

        canvas.set_draw_color(artwork_background_color(ctx));
        canvas.fill_rect(Rect::new(0, 0, scene_w, top_h))?;

        canvas.set_draw_color(metadata_background_color(ctx));
        canvas.fill_rect(Rect::new(0, top_h as i32, scene_w, bottom_h))?;

        if let (Some(texture), Some(target)) = (artwork_texture, self.artwork_rect) {
            canvas.copy(texture, None, target)?;
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
            .third
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .detail
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

    let mut window_builder = video.window(
        &ctx.config.display.window_title,
        preset.width,
        preset.height,
    );
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

    log_info(&ctx, &format!("Selected font theme: {}", selected_theme));

    let mut title_font = ttf_ctx
        .load_font(title_font_path, title_font_size)
        .map_err(|e| format!("Failed to load title font from {}: {e}", title_font_path))?;

    let mut body_font = ttf_ctx
        .load_font(body_font_path, body_font_size)
        .map_err(|e| format!("Failed to load body font from {}: {e}", body_font_path))?;

    let mut loaded_font_theme = selected_theme;

    let scene_w = preset.width;
    let scene_h = preset.height;
    let top_h = ((scene_h as f32) * preset.top_panel_ratio) as u32;
    let bottom_h = scene_h - top_h;

    let mut event_pump = sdl.event_pump()?;
    let mut loaded_version: u64 = u64::MAX;
    let mut artwork_texture: Option<Texture<'_>> = None;
    let mut visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
    let mut last_canvas_size: Option<(u32, u32)> = None;
    let mut display_peak = 0.0f32;
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

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => running.store(false, Ordering::SeqCst),
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
                ctx.config.visualizer.gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain,
            );

            let right = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.right_y_offset,
                ctx.config.visualizer.y_scale,
                ctx.config.visualizer.gain,
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
                ctx.config.visualizer.gain,
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
            let top_only = ctx
                .config
                .visualizer
                .spectrum
                .render_style
                .eq_ignore_ascii_case("top_only");
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

        state.meter.level = live_level;
        state.meter.peak = if ctx.config.visualizer.peak_hold {
            display_peak
        } else {
            live_level
        };

        state.visualizer.enabled = ctx.config.visualizer.enabled;
        state.visualizer.mode = visualizer_mode_from_config(&ctx.config.visualizer.mode);
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

        if state.version != loaded_version {
            if !state.artwork_path.is_empty() && Path::new(&state.artwork_path).exists() {
                match texture_creator.load_texture(&state.artwork_path) {
                    Ok(texture) => {
                        artwork_texture = Some(texture);
                        visualizer_colors =
                            visualizer_colors_for_artwork(&ctx, Some(&state.artwork_path));
                        loaded_version = state.version;
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
                        visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                    }
                }
            } else {
                artwork_texture = None;
                visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                loaded_version = state.version;
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

            if selected_theme != loaded_font_theme {
                log_info(
                    &ctx,
                    &format!(
                        "Changing font theme from '{}' to '{}' for genre='{}' released='{}'",
                        loaded_font_theme, selected_theme, state.genre, state.released
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
                top_h,
            )?;

            let artwork_rect = artwork_texture
                .as_ref()
                .map(|texture| compute_artwork_rect(texture.query(), scene_w, top_h));

            let cache = StaticSceneCache {
                version: state.version,
                text,
                artwork_rect,
            };

            canvas
                .with_texture_canvas(&mut static_scene_texture, |tex_canvas| {
                    let _ = cache.draw_static_scene(
                        tex_canvas,
                        &ctx,
                        artwork_texture.as_ref(),
                        scene_w,
                        top_h,
                        bottom_h,
                    );
                })
                .map_err(|e| e.to_string())?;

            static_scene_cache = Some(cache);
            text_scroll_started_at = Instant::now();
            log_debug(
                &ctx,
                &format!("Rebuilt static scene for version {}", state.version),
            );
        }

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
            let vis_h = ctx.config.visualizer.height.min(bottom_h.saturating_sub(8));

            let vis_y_scene =
                (scene_h as i32) - (vis_h as i32) - (ctx.config.visualizer.padding as i32);
            let vis_x_scene = ctx.config.visualizer.padding as i32;
            let vis_w_scene = scene_w.saturating_sub(ctx.config.visualizer.padding * 2);

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
            )?;
        }

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
