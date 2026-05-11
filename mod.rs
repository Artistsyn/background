//! Background system plugin for Quartz.
//!
//! Provides `LayeredBackground`, `BackgroundLayer`, and `BackgroundPlugin`
//! for declarative, composited scene backgrounds with smooth transitions and
//! an optional disk cache.
//!
//! # Usage
//! ```no_run
//! use quartz::prelude::*;
//!
//! let mut canvas = Canvas::new(...);
//!
//! let bg = LayeredBackground::new()
//!     .with_layer(BackgroundLayer::GradientVertical { top: (8, 26, 74), bottom: (104, 194, 255) })
//!     .with_layer(BackgroundLayer::Starfield { density: 300, seed: 0xCAFE_BABE,
//!         size_range: (0, 1), brightness_range: (100, 255), vertical_fade: Some(200), scale: None });
//!
//! let mut plugin = BackgroundPlugin::new(1280, 720);
//! plugin.set_background("sky", bg, None);
//! canvas.add_plugin(plugin);
//! ```

use std::collections::HashMap;
use image::RgbaImage;
use crate::{Canvas, plugin::QuartzPlugin};
use image_gen::{ImageGen, ResizeFilter};

// ── BackgroundLayer ───────────────────────────────────────────────────────────

/// One layer in a background stack.
///
/// Layers are composited bottom-to-top (index 0 is rendered first / behind).
#[derive(Clone)]
pub enum BackgroundLayer {
    /// Solid colour fill.
    Solid { color: (u8, u8, u8) },

    /// Vertical linear gradient.
    GradientVertical { top: (u8, u8, u8), bottom: (u8, u8, u8) },

    /// Horizontal linear gradient.
    GradientHorizontal { left: (u8, u8, u8), right: (u8, u8, u8) },

    /// Four-corner gradient with bilinear interpolation.
    GradientFourCorner {
        top_left:     (u8, u8, u8),
        top_right:    (u8, u8, u8),
        bottom_left:  (u8, u8, u8),
        bottom_right: (u8, u8, u8),
    },

    /// Procedural starfield on a transparent background.
    ///
    /// Set `vertical_fade` to fade stars out toward the horizon.
    /// Set `scale` < 1.0 to draw stars at a fraction of full size (distant look).
    Starfield {
        density:          u32,
        seed:             u64,
        size_range:       (u32, u32),
        brightness_range: (u8, u8),
        /// Fade to transparent below this many pixels from the top.
        vertical_fade:    Option<u32>,
        /// Draw starfield scaled to this fraction of full size, centered.
        scale:            Option<f32>,
    },

    /// Cloud/nebula texture using simple value noise.
    Nebula { color: (u8, u8, u8), density: f32, seed: u64 },

    /// An image asset loaded from bytes and resized to background dimensions.
    Image { bytes: &'static [u8], filter: ResizeFilter },

    /// A caller-supplied `RgbaImage`. Not eligible for disk caching.
    Raw(RgbaImage),
}

// ── LayeredBackground ─────────────────────────────────────────────────────────

/// A complete background composed of stacked layers.
///
/// Call `build()` once to composite all layers into a final `RgbaImage`.
pub struct LayeredBackground {
    layers:        Vec<BackgroundLayer>,
    tint:          (u8, u8, u8),
    flip_vertical: bool,
    pre_scale:     Option<f32>,
}

impl LayeredBackground {
    /// Creates an empty background descriptor.
    pub fn new() -> Self {
        Self {
            layers:        Vec::new(),
            tint:          (255, 255, 255),
            flip_vertical: false,
            pre_scale:     None,
        }
    }

    /// Adds a layer on top of existing layers. Last added = topmost.
    pub fn with_layer(mut self, layer: BackgroundLayer) -> Self {
        self.layers.push(layer);
        self
    }

    /// Applies a global tint after all layers are composited.
    /// `(255, 255, 255)` is identity (no change).
    pub fn with_tint(mut self, tint: (u8, u8, u8)) -> Self {
        self.tint = tint;
        self
    }

    /// Flips the final image vertically (for reverse-gravity zones).
    pub fn flipped(mut self) -> Self {
        self.flip_vertical = true;
        self
    }

    /// Scales the background before output. Values < 1.0 give a "zoomed-out" look.
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.pre_scale = Some(scale);
        self
    }

    /// Composites all layers and returns the final `RgbaImage`.
    ///
    /// This is called once at scene construction time, not per frame.
    pub fn build(self, width: u32, height: u32) -> RgbaImage {
        let base = ImageGen::solid(width, height, (0, 0, 0, 255));
        let composited = self.layers.iter().fold(base, |acc, layer| {
            let layer_img = render_layer(layer, width, height);
            ImageGen::composite(&acc, &layer_img)
        });

        let scaled = if let Some(scale) = self.pre_scale {
            let sw = ((width  as f32 * scale) as u32).max(1);
            let sh = ((height as f32 * scale) as u32).max(1);
            let shrunk = ImageGen::resize(&composited, sw, sh, ResizeFilter::Lanczos3);
            let mut canvas = ImageGen::solid(width, height, (0, 0, 0, 255));
            let ox = (width.saturating_sub(sw))  / 2;
            let oy = (height.saturating_sub(sh)) / 2;
            image::imageops::overlay(&mut canvas, &shrunk, ox as i64, oy as i64);
            canvas
        } else {
            composited
        };

        let tinted = if self.tint != (255, 255, 255) {
            ImageGen::tint(&scaled, self.tint)
        } else {
            scaled
        };

        if self.flip_vertical {
            ImageGen::flip_vertical(&tinted)
        } else {
            tinted
        }
    }
}

fn render_layer(layer: &BackgroundLayer, width: u32, height: u32) -> RgbaImage {
    match layer {
        BackgroundLayer::Solid { color } =>
            ImageGen::solid(width, height, (color.0, color.1, color.2, 255)),

        BackgroundLayer::GradientVertical { top, bottom } =>
            ImageGen::gradient_vertical(width, height, *top, *bottom),

        BackgroundLayer::GradientHorizontal { left, right } =>
            ImageGen::gradient_horizontal(width, height, *left, *right),

        BackgroundLayer::GradientFourCorner { top_left, top_right, bottom_left, bottom_right } =>
            ImageGen::gradient_four_corner(width, height, *top_left, *top_right, *bottom_left, *bottom_right),

        BackgroundLayer::Starfield { density, seed, size_range, brightness_range, vertical_fade, scale } => {
            let stars = ImageGen::starfield(width, height, *density, *seed, *size_range, *brightness_range);
            let base  = ImageGen::solid(width, height, (0, 0, 0, 0));
            let composed = match (vertical_fade, scale) {
                (Some(fade), Some(s)) => {
                    let faded = ImageGen::composite_with_vertical_fade(&base, &stars, *fade);
                    ImageGen::composite_scaled(&ImageGen::solid(width, height, (0,0,0,0)), &faded, *s)
                }
                (Some(fade), None) =>
                    ImageGen::composite_with_vertical_fade(&base, &stars, *fade),
                (None, Some(s)) =>
                    ImageGen::composite_scaled(&base, &stars, *s),
                (None, None) => stars,
            };
            composed
        }

        BackgroundLayer::Nebula { color, density, seed } =>
            ImageGen::nebula(width, height, *color, *density, *seed),

        BackgroundLayer::Image { bytes, filter } =>
            ImageGen::load_and_resize(bytes, width, height, *filter),

        BackgroundLayer::Raw(img) => {
            if img.dimensions() == (width, height) {
                img.clone()
            } else {
                ImageGen::resize(img, width, height, ResizeFilter::Bilinear)
            }
        }
    }
}

// ── BackgroundPlugin ──────────────────────────────────────────────────────────

/// Active transition state between two named backgrounds.
struct Transition {
    from_key: String,
    to_key:   String,
    elapsed:  f32,
    duration: f32,
}

/// Quartz plugin that manages named scene backgrounds and smooth transitions.
///
/// Register once with `canvas.add_plugin(BackgroundPlugin::new(w, h))`.
///
/// Control via `Action::RunPlugin`:
/// - `"background:set"` with data `"<key>"` — instant swap
/// - `"background:transition"` with data `"<from>,<to>,<duration_secs>"` — crossfade
pub struct BackgroundPlugin {
    width:       u32,
    height:      u32,
    /// Named, pre-built background images.
    backgrounds: HashMap<String, RgbaImage>,
    /// Currently displayed background key.
    current:     Option<String>,
    /// Active transition, if any.
    transition:  Option<Transition>,
    /// Cached blended frame for the current transition step.
    blend_frame: Option<RgbaImage>,
}

impl BackgroundPlugin {
    /// Creates a new plugin for backgrounds of the given pixel dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            backgrounds: HashMap::new(),
            current:     None,
            transition:  None,
            blend_frame: None,
        }
    }

    /// Adds (or replaces) a named background from a `LayeredBackground` descriptor.
    ///
    /// Pass a `cache_dir` to enable disk caching so the image is generated only once.
    pub fn set_background(
        &mut self,
        key: impl Into<String>,
        bg: LayeredBackground,
        cache_dir: Option<&str>,
    ) {
        let key = key.into();
        let img = if let Some(dir) = cache_dir {
            load_or_build_cached(&key, bg, self.width, self.height, dir)
        } else {
            bg.build(self.width, self.height)
        };
        self.backgrounds.insert(key, img);
    }

    /// Instantly switches the active background to `key`.
    pub fn show(&mut self, key: &str) {
        if self.backgrounds.contains_key(key) {
            self.current    = Some(key.to_string());
            self.transition = None;
            self.blend_frame = None;
        }
    }

    /// Starts a crossfade transition between `from` and `to` over `duration` seconds.
    pub fn begin_transition(&mut self, from: &str, to: &str, duration: f32) {
        if self.backgrounds.contains_key(from) && self.backgrounds.contains_key(to) {
            self.transition = Some(Transition {
                from_key: from.to_string(),
                to_key:   to.to_string(),
                elapsed:  0.0,
                duration: duration.max(0.001),
            });
        }
    }

    /// Returns the image currently shown (after applying any transition blend).
    pub fn current_image(&self) -> Option<&RgbaImage> {
        if let Some(blend) = &self.blend_frame {
            return Some(blend);
        }
        self.current.as_ref().and_then(|k| self.backgrounds.get(k))
    }
}

impl QuartzPlugin for BackgroundPlugin {
    fn name(&self) -> &str { "background" }

    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }

    fn on_update(&mut self, _canvas: &mut Canvas, dt: f32) {
        let Some(tr) = self.transition.as_mut() else { return };
        tr.elapsed += dt;
        let t = (tr.elapsed / tr.duration).clamp(0.0, 1.0);

        let from_img = self.backgrounds.get(&tr.from_key);
        let to_img   = self.backgrounds.get(&tr.to_key);

        if let (Some(a), Some(b)) = (from_img, to_img) {
            self.blend_frame = Some(ImageGen::blend(a, b, t));
        }

        if tr.elapsed >= tr.duration {
            let to_key = tr.to_key.clone();
            self.current    = Some(to_key);
            self.transition = None;
            self.blend_frame = None;
        }
    }

    fn on_action(&mut self, _canvas: &mut Canvas, data: &str) -> bool {
        // data format: "<sub-command>:<args>"
        if let Some(rest) = data.strip_prefix("set:") {
            self.show(rest.trim());
            return true;
        }
        if let Some(rest) = data.strip_prefix("transition:") {
            // format: "from,to,duration"
            let parts: Vec<&str> = rest.splitn(3, ',').collect();
            if parts.len() == 3 {
                let from = parts[0].trim();
                let to   = parts[1].trim();
                if let Ok(secs) = parts[2].trim().parse::<f32>() {
                    self.begin_transition(from, to, secs);
                    return true;
                }
            }
        }
        false
    }
}

// ── Disk cache ────────────────────────────────────────────────────────────────

fn load_or_build_cached(
    key: &str, bg: LayeredBackground,
    width: u32, height: u32,
    dir: &str,
) -> RgbaImage {
    let hash = fnv1a_hash(key, width, height);
    let filename = format!("{}/{}__{:016x}.png", dir, key.replace(['/', '\\', ':'], "_"), hash);

    if std::path::Path::new(&filename).exists() {
        if let Ok(img) = image::open(&filename) {
            return img.to_rgba8();
        }
    }

    let generated = bg.build(width, height);

    // Best-effort save — silently skip if the directory can't be written.
    let _ = std::fs::create_dir_all(dir);
    let _ = generated.save(&filename);

    generated
}

/// Simple FNV-1a 64-bit hash of a string + dimensions.
fn fnv1a_hash(key: &str, width: u32, height: u32) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME:  u64 = 1099511628211;
    let mut h = OFFSET;
    for b in key.bytes().chain(width.to_le_bytes()).chain(height.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

// ── image_gen — self-contained image generation utilities ─────────────────────
//
// This module is private to the background plugin. It has no dependencies
// beyond the `image` crate already required by Quartz.
mod image_gen {
    use image::{RgbaImage, Rgba, imageops};

    pub struct ImageGen;

    #[allow(dead_code)]
    impl ImageGen {
        pub fn gradient_vertical(
            width: u32, height: u32,
            top: (u8, u8, u8), bottom: (u8, u8, u8),
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            for y in 0..height {
                let t = y as f32 / (height - 1).max(1) as f32;
                let color = (lerp_u8(top.0, bottom.0, t), lerp_u8(top.1, bottom.1, t), lerp_u8(top.2, bottom.2, t));
                for x in 0..width { img.put_pixel(x, y, Rgba([color.0, color.1, color.2, 255])); }
            }
            img
        }

        pub fn gradient_horizontal(
            width: u32, height: u32,
            left: (u8, u8, u8), right: (u8, u8, u8),
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            for x in 0..width {
                let t = x as f32 / (width - 1).max(1) as f32;
                let color = (lerp_u8(left.0, right.0, t), lerp_u8(left.1, right.1, t), lerp_u8(left.2, right.2, t));
                for y in 0..height { img.put_pixel(x, y, Rgba([color.0, color.1, color.2, 255])); }
            }
            img
        }

        pub fn gradient_four_corner(
            width: u32, height: u32,
            top_left: (u8, u8, u8), top_right: (u8, u8, u8),
            bottom_left: (u8, u8, u8), bottom_right: (u8, u8, u8),
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            for y in 0..height {
                let ty = y as f32 / (height - 1).max(1) as f32;
                for x in 0..width {
                    let tx = x as f32 / (width - 1).max(1) as f32;
                    let r = bilerp_u8(top_left.0, top_right.0, bottom_left.0, bottom_right.0, tx, ty);
                    let g = bilerp_u8(top_left.1, top_right.1, bottom_left.1, bottom_right.1, tx, ty);
                    let b = bilerp_u8(top_left.2, top_right.2, bottom_left.2, bottom_right.2, tx, ty);
                    img.put_pixel(x, y, Rgba([r, g, b, 255]));
                }
            }
            img
        }

        pub fn gradient_radial(
            width: u32, height: u32,
            center: (u8, u8, u8), edge: (u8, u8, u8),
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            let cx = width as f32 * 0.5;
            let cy = height as f32 * 0.5;
            let max_dist = (cx * cx + cy * cy).sqrt();
            for y in 0..height {
                for x in 0..width {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let t = ((dx * dx + dy * dy).sqrt() / max_dist).clamp(0.0, 1.0);
                    img.put_pixel(x, y, Rgba([lerp_u8(center.0, edge.0, t), lerp_u8(center.1, edge.1, t), lerp_u8(center.2, edge.2, t), 255]));
                }
            }
            img
        }

        pub fn solid(width: u32, height: u32, color: (u8, u8, u8, u8)) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            for pixel in img.pixels_mut() { *pixel = Rgba([color.0, color.1, color.2, color.3]); }
            img
        }

        pub fn starfield(
            width: u32, height: u32,
            density: u32, seed: u64,
            size_range: (u32, u32),
            brightness_range: (u8, u8),
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            let mut rng = LcgRng::new(seed);
            for _ in 0..density {
                let x = rng.next_u32() % width;
                let y = rng.next_u32() % height;
                let brange = brightness_range.1.saturating_sub(brightness_range.0);
                let brightness = brightness_range.0 + (rng.next_u32() % (brange as u32 + 1)) as u8;
                let srange = (size_range.1.saturating_sub(size_range.0) + 1).max(1);
                let radius = size_range.0 + rng.next_u32() % srange;
                for dy in 0..=(radius * 2) {
                    for dx in 0..=(radius * 2) {
                        let px = x as i32 + dx as i32 - radius as i32;
                        let py = y as i32 + dy as i32 - radius as i32;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 { continue; }
                        let dist = ((dx as f32 - radius as f32).powi(2) + (dy as f32 - radius as f32).powi(2)).sqrt();
                        if dist <= radius as f32 {
                            let alpha = (brightness as f32 * (1.0 - dist / (radius as f32).max(0.01))) as u8;
                            img.put_pixel(px as u32, py as u32, Rgba([255, 255, 255, alpha]));
                        }
                    }
                }
            }
            img
        }

        pub fn nebula(
            width: u32, height: u32,
            color: (u8, u8, u8), density: f32, seed: u64,
        ) -> RgbaImage {
            let mut img = RgbaImage::new(width, height);
            let scale = 0.004_f32;
            for y in 0..height {
                for x in 0..width {
                    let n = smooth_noise(x as f32 * scale, y as f32 * scale, seed);
                    let alpha = ((n * density * 200.0) as u8).min(180);
                    img.put_pixel(x, y, Rgba([color.0, color.1, color.2, alpha]));
                }
            }
            img
        }

        pub fn resize(src: &RgbaImage, width: u32, height: u32, filter: ResizeFilter) -> RgbaImage {
            imageops::resize(src, width, height, filter.to_image_filter())
        }

        pub fn flip_vertical(src: &RgbaImage) -> RgbaImage { imageops::flip_vertical(src) }
        pub fn flip_horizontal(src: &RgbaImage) -> RgbaImage { imageops::flip_horizontal(src) }

        pub fn composite(base: &RgbaImage, overlay: &RgbaImage) -> RgbaImage {
            let (w, h) = base.dimensions();
            let mut out = RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    out.put_pixel(x, y, alpha_over(*base.get_pixel(x, y), *overlay.get_pixel(x, y)));
                }
            }
            out
        }

        pub fn composite_with_vertical_fade(base: &RgbaImage, overlay: &RgbaImage, fade_height: u32) -> RgbaImage {
            let (w, h) = base.dimensions();
            let mut out = base.clone();
            for y in 0..h.min(overlay.height()) {
                let alpha_scale = if y < fade_height { 1.0 - (y as f32 / fade_height as f32) } else { 0.0 };
                for x in 0..w.min(overlay.width()) {
                    let b = *base.get_pixel(x, y);
                    let mut o = *overlay.get_pixel(x, y);
                    o[3] = (o[3] as f32 * alpha_scale) as u8;
                    out.put_pixel(x, y, alpha_over(b, o));
                }
            }
            out
        }

        pub fn composite_scaled(base: &RgbaImage, overlay: &RgbaImage, scale: f32) -> RgbaImage {
            let (w, h) = base.dimensions();
            let sw = ((w as f32 * scale) as u32).max(1);
            let sh = ((h as f32 * scale) as u32).max(1);
            let scaled = imageops::resize(overlay, sw, sh, imageops::FilterType::Lanczos3);
            let ox = (w.saturating_sub(sw)) / 2;
            let oy = (h.saturating_sub(sh)) / 2;
            let mut out = base.clone();
            for sy in 0..sh {
                for sx in 0..sw {
                    if ox + sx < w && oy + sy < h {
                        let b = *out.get_pixel(ox + sx, oy + sy);
                        let o = *scaled.get_pixel(sx, sy);
                        out.put_pixel(ox + sx, oy + sy, alpha_over(b, o));
                    }
                }
            }
            out
        }

        pub fn blend(a: &RgbaImage, b: &RgbaImage, t: f32) -> RgbaImage {
            let (w, h) = a.dimensions();
            let t = t.clamp(0.0, 1.0);
            let mut out = RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    let pa = a.get_pixel(x, y);
                    let pb = b.get_pixel(x, y);
                    out.put_pixel(x, y, Rgba([lerp_u8(pa[0], pb[0], t), lerp_u8(pa[1], pb[1], t), lerp_u8(pa[2], pb[2], t), lerp_u8(pa[3], pb[3], t)]));
                }
            }
            out
        }

        pub fn tint(src: &RgbaImage, tint: (u8, u8, u8)) -> RgbaImage {
            let mut out = src.clone();
            for pixel in out.pixels_mut() {
                pixel[0] = ((pixel[0] as u16 * tint.0 as u16) / 255) as u8;
                pixel[1] = ((pixel[1] as u16 * tint.1 as u16) / 255) as u8;
                pixel[2] = ((pixel[2] as u16 * tint.2 as u16) / 255) as u8;
            }
            out
        }

        pub fn load_and_resize(bytes: &[u8], width: u32, height: u32, filter: ResizeFilter) -> RgbaImage {
            let src = image::load_from_memory(bytes)
                .expect("ImageGen::load_and_resize: failed to decode image bytes")
                .to_rgba8();
            Self::resize(&src, width, height, filter)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ResizeFilter { Nearest, Bilinear, Bicubic, Lanczos3 }

    impl ResizeFilter {
        fn to_image_filter(self) -> imageops::FilterType {
            match self {
                ResizeFilter::Nearest  => imageops::FilterType::Nearest,
                ResizeFilter::Bilinear => imageops::FilterType::Triangle,
                ResizeFilter::Bicubic  => imageops::FilterType::CatmullRom,
                ResizeFilter::Lanczos3 => imageops::FilterType::Lanczos3,
            }
        }
    }

    #[inline] fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
        (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
    }
    #[inline] fn bilerp_u8(tl: u8, tr: u8, bl: u8, br: u8, tx: f32, ty: f32) -> u8 {
        lerp_u8(lerp_u8(tl, tr, tx), lerp_u8(bl, br, tx), ty)
    }
    fn alpha_over(base: Rgba<u8>, over: Rgba<u8>) -> Rgba<u8> {
        let ao = over[3] as f32 / 255.0;
        let ab = base[3] as f32 / 255.0;
        let alpha_out = ao + ab * (1.0 - ao);
        if alpha_out < 1e-6 { return Rgba([0, 0, 0, 0]); }
        Rgba([
            ((over[0] as f32 * ao + base[0] as f32 * ab * (1.0 - ao)) / alpha_out) as u8,
            ((over[1] as f32 * ao + base[1] as f32 * ab * (1.0 - ao)) / alpha_out) as u8,
            ((over[2] as f32 * ao + base[2] as f32 * ab * (1.0 - ao)) / alpha_out) as u8,
            (alpha_out * 255.0) as u8,
        ])
    }

    struct LcgRng { state: u64 }
    impl LcgRng {
        fn new(seed: u64) -> Self { Self { state: seed ^ 0x123456789ABCDEF0 } }
        fn next_u32(&mut self) -> u32 {
            self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.state >> 33) as u32
        }
        fn next_f32(&mut self) -> f32 { self.next_u32() as f32 / u32::MAX as f32 }
    }

    fn smooth_noise(x: f32, y: f32, seed: u64) -> f32 {
        let xi = x.floor() as i32;
        let yi = y.floor() as i32;
        let (xf, yf) = (x - xi as f32, y - yi as f32);
        let v00 = hash_to_f32(xi,   yi,   seed);
        let v10 = hash_to_f32(xi+1, yi,   seed);
        let v01 = hash_to_f32(xi,   yi+1, seed);
        let v11 = hash_to_f32(xi+1, yi+1, seed);
        let (ux, uy) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
        let top    = v00 + (v10 - v00) * ux;
        let bottom = v01 + (v11 - v01) * ux;
        top + (bottom - top) * uy
    }
    fn hash_to_f32(x: i32, y: i32, seed: u64) -> f32 {
        let mut rng = LcgRng::new(seed ^ (x as u64).wrapping_mul(374761393) ^ (y as u64).wrapping_mul(668265263));
        rng.next_f32()
    }
}
