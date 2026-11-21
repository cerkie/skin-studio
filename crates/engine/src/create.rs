//! Procedural SDF osu! skin element creator: resolution-independent 32-bit premultiplied float layer composer.
//! Renders crisp analytical shapes, gradients, strokes, inner/outer glows, and blend modes in parallel with rayon,
//! producing `@2x` assets and zero-ringing area-averaged `1x` pairs with zero banding by construction.
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::img::Img;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ElementKind {
    HitCircle,
    HitCircleOverlay,
    ApproachCircle,
    Cursor,
    CursorMiddle,
    CursorTrail,
    SliderBall,
    SliderFollowCircle,
    ReverseArrow,
    SliderTick,
    Custom,
}

impl ElementKind {
    pub const ALL: [ElementKind; 11] = [
        ElementKind::HitCircle,
        ElementKind::HitCircleOverlay,
        ElementKind::ApproachCircle,
        ElementKind::Cursor,
        ElementKind::CursorMiddle,
        ElementKind::CursorTrail,
        ElementKind::SliderBall,
        ElementKind::SliderFollowCircle,
        ElementKind::ReverseArrow,
        ElementKind::SliderTick,
        ElementKind::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ElementKind::HitCircle => "Hit circle",
            ElementKind::HitCircleOverlay => "Hit circle overlay",
            ElementKind::ApproachCircle => "Approach circle",
            ElementKind::Cursor => "Cursor",
            ElementKind::CursorMiddle => "Cursor middle",
            ElementKind::CursorTrail => "Cursor trail",
            ElementKind::SliderBall => "Slider ball",
            ElementKind::SliderFollowCircle => "Slider follow circle",
            ElementKind::ReverseArrow => "Reverse arrow",
            ElementKind::SliderTick => "Slider tick",
            ElementKind::Custom => "Custom element",
        }
    }

    /// Default base filename (without `@2x.png` / `.png`).
    pub fn file_stem(self) -> &'static str {
        match self {
            ElementKind::HitCircle => "hitcircle",
            ElementKind::HitCircleOverlay => "hitcircleoverlay",
            ElementKind::ApproachCircle => "approachcircle",
            ElementKind::Cursor => "cursor",
            ElementKind::CursorMiddle => "cursormiddle",
            ElementKind::CursorTrail => "cursortrail",
            ElementKind::SliderBall => "sliderb0",
            ElementKind::SliderFollowCircle => "sliderfollowcircle",
            ElementKind::ReverseArrow => "reversearrow",
            ElementKind::SliderTick => "sliderscorepoint",
            ElementKind::Custom => "custom-element",
        }
    }

    /// Canonical `@2x` canvas dimensions in pixels (2× the official osu! wiki Suggested SD Size).
    pub fn default_size_2x(self) -> (usize, usize) {
        match self {
            // 128x128 SD (118x118 circle inside) -> 256x256 @2x (236x236 circle inside)
            ElementKind::HitCircle | ElementKind::HitCircleOverlay | ElementKind::ReverseArrow => (256, 256),
            // 126x126 SD -> 252x252 @2x
            ElementKind::ApproachCircle => (252, 252),
            // 118x118 SD -> 236x236 @2x
            ElementKind::SliderBall => (236, 236),
            // 256x256 SD -> 512x512 @2x
            ElementKind::SliderFollowCircle => (512, 512),
            // 16x16 SD -> 32x32 @2x
            ElementKind::SliderTick => (32, 32),
            // Standard osu! cursor sizes @2x (64 / 16 / 32 SD)
            ElementKind::Cursor => (128, 128),
            ElementKind::CursorMiddle => (32, 32),
            ElementKind::CursorTrail => (64, 64),
            ElementKind::Custom => (256, 256),
        }
    }

    /// Whether osu! multiplies this element by the active combo color in-game.
    pub fn is_combo_tinted(self) -> bool {
        matches!(self, ElementKind::HitCircle | ElementKind::ApproachCircle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    Normal,
    Add,
    Screen,
    Multiply,
    Erase,
}

impl BlendMode {
    pub const ALL: [BlendMode; 5] = [BlendMode::Normal, BlendMode::Add, BlendMode::Screen, BlendMode::Multiply, BlendMode::Erase];
    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Additive",
            BlendMode::Screen => "Screen",
            BlendMode::Multiply => "Multiply",
            BlendMode::Erase => "Erase (cutout)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeAlign {
    Inside,
    Center,
    Outside,
}

impl StrokeAlign {
    pub const ALL: [StrokeAlign; 3] = [StrokeAlign::Inside, StrokeAlign::Center, StrokeAlign::Outside];
    pub fn name(self) -> &'static str {
        match self {
            StrokeAlign::Inside => "Inside",
            StrokeAlign::Center => "Center",
            StrokeAlign::Outside => "Outside",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Disc { radius: f32 },
    Ring { radius: f32, thickness: f32 },
    RoundedRect { width: f32, height: f32, corner: f32 },
    Polygon { sides: u8, radius: f32, corner: f32 },
    Chevron { size: f32, thickness: f32, spread: f32 },
    Crosshair { length: f32, thickness: f32, gap: f32 },
}

impl Shape {
    pub fn kind_name(self) -> &'static str {
        match self {
            Shape::Disc { .. } => "Disc",
            Shape::Ring { .. } => "Ring",
            Shape::RoundedRect { .. } => "Rounded rect",
            Shape::Polygon { .. } => "Polygon",
            Shape::Chevron { .. } => "Chevron",
            Shape::Crosshair { .. } => "Crosshair",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FillStyle {
    Solid { color: [f32; 3], alpha: f32 },
    Radial { inner: [f32; 3], inner_a: f32, outer: [f32; 3], outer_a: f32, power: f32 },
    Linear { top: [f32; 3], top_a: f32, bottom: [f32; 3], bottom_a: f32, angle: f32 },
}

impl FillStyle {
    pub fn kind_index(self) -> usize {
        match self {
            FillStyle::Solid { .. } => 0,
            FillStyle::Radial { .. } => 1,
            FillStyle::Linear { .. } => 2,
        }
    }
    pub fn kind_name(self) -> &'static str {
        match self {
            FillStyle::Solid { .. } => "Solid",
            FillStyle::Radial { .. } => "Radial gradient",
            FillStyle::Linear { .. } => "Linear gradient",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeStyle {
    pub enabled: bool,
    pub width: f32,
    pub color: [f32; 3],
    pub opacity: f32,
    pub align: StrokeAlign,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle { enabled: false, width: 4.0, color: [1.0, 1.0, 1.0], opacity: 1.0, align: StrokeAlign::Inside }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlowStyle {
    pub enabled: bool,
    pub radius: f32,
    pub color: [f32; 3],
    pub intensity: f32,
    pub inner: bool,
}

impl Default for GlowStyle {
    fn default() -> Self {
        GlowStyle { enabled: false, radius: 18.0, color: [1.0, 1.0, 1.0], intensity: 0.6, inner: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub offset: [f32; 2],
    pub rotation: f32,
    pub feather: f32,
    pub shape: Shape,
    pub fill: FillStyle,
    pub stroke: StrokeStyle,
    pub glow: GlowStyle,
}

impl Layer {
    pub fn new(name: &str, shape: Shape) -> Self {
        Layer {
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            offset: [0.0, 0.0],
            rotation: 0.0,
            feather: 0.0,
            shape,
            fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
            stroke: StrokeStyle::default(),
            glow: GlowStyle::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElementDoc {
    pub kind: ElementKind,
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub layers: Vec<Layer>,
}

impl ElementDoc {
    /// Build a polished, osu!-ready starter template for `kind`.
    pub fn template(kind: ElementKind) -> Self {
        let (width, height) = kind.default_size_2x();
        let name = kind.file_stem().to_string();
        let layers = match kind {
            // Official osu! hitcircle: 118 px circle inside 128 px SD canvas = 236 px diameter (118 px radius) inside 256 px @2x canvas.
            ElementKind::HitCircle => vec![
                Layer {
                    name: "Body fill".into(),
                    shape: Shape::Disc { radius: 118.0 },
                    fill: FillStyle::Radial { inner: [0.92, 0.92, 0.92], inner_a: 0.94, outer: [0.58, 0.58, 0.58], outer_a: 0.88, power: 1.4 },
                    glow: GlowStyle { enabled: true, radius: 16.0, color: [1.0, 1.0, 1.0], intensity: 0.35, inner: true },
                    ..Layer::new("Body fill", Shape::Disc { radius: 118.0 })
                },
                Layer {
                    name: "Inner ring".into(),
                    opacity: 0.85,
                    shape: Shape::Ring { radius: 113.0, thickness: 10.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    ..Layer::new("Inner ring", Shape::Ring { radius: 113.0, thickness: 10.0 })
                },
            ],
            // Official osu! hitcircleoverlay: outer edge at 118 px radius (236 px diameter) inside 256 px @2x canvas.
            ElementKind::HitCircleOverlay => vec![
                Layer {
                    name: "Outer glow".into(),
                    blend: BlendMode::Add,
                    shape: Shape::Ring { radius: 113.0, thickness: 6.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 0.0 },
                    glow: GlowStyle { enabled: true, radius: 10.0, color: [1.0, 1.0, 1.0], intensity: 0.45, inner: false },
                    ..Layer::new("Outer glow", Shape::Ring { radius: 113.0, thickness: 6.0 })
                },
                Layer {
                    name: "Border ring".into(),
                    shape: Shape::Ring { radius: 113.0, thickness: 10.0 },
                    fill: FillStyle::Linear { top: [1.0, 1.0, 1.0], top_a: 1.0, bottom: [0.82, 0.84, 0.92], bottom_a: 1.0, angle: 90.0 },
                    ..Layer::new("Border ring", Shape::Ring { radius: 113.0, thickness: 10.0 })
                },
            ],
            // Official osu! approachcircle: 252x252 @2x (126x126 SD); ring outer edge lands flush at 118 px radius on hit.
            ElementKind::ApproachCircle => vec![Layer {
                name: "Approach ring".into(),
                shape: Shape::Ring { radius: 115.0, thickness: 8.0 },
                fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                glow: GlowStyle { enabled: true, radius: 8.0, color: [1.0, 1.0, 1.0], intensity: 0.35, inner: false },
                ..Layer::new("Approach ring", Shape::Ring { radius: 115.0, thickness: 8.0 })
            }],
            ElementKind::Cursor => vec![
                Layer {
                    name: "Soft glow".into(),
                    blend: BlendMode::Add,
                    shape: Shape::Disc { radius: 24.0 },
                    fill: FillStyle::Radial { inner: [1.0, 0.52, 0.78], inner_a: 0.75, outer: [0.65, 0.35, 1.0], outer_a: 0.0, power: 1.2 },
                    glow: GlowStyle { enabled: true, radius: 20.0, color: [1.0, 0.48, 0.76], intensity: 0.75, inner: false },
                    ..Layer::new("Soft glow", Shape::Disc { radius: 24.0 })
                },
                Layer {
                    name: "Core ring".into(),
                    shape: Shape::Ring { radius: 22.0, thickness: 5.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    ..Layer::new("Core ring", Shape::Ring { radius: 22.0, thickness: 5.0 })
                },
                Layer {
                    name: "Center dot".into(),
                    shape: Shape::Disc { radius: 5.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    ..Layer::new("Center dot", Shape::Disc { radius: 5.0 })
                },
            ],
            ElementKind::CursorMiddle => vec![Layer {
                name: "Center dot".into(),
                shape: Shape::Disc { radius: 6.0 },
                fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                stroke: StrokeStyle { enabled: true, width: 2.0, color: [0.15, 0.12, 0.22], opacity: 0.85, align: StrokeAlign::Outside },
                ..Layer::new("Center dot", Shape::Disc { radius: 6.0 })
            }],
            ElementKind::CursorTrail => vec![Layer {
                name: "Trail glow".into(),
                shape: Shape::Disc { radius: 24.0 },
                fill: FillStyle::Radial { inner: [1.0, 0.56, 0.82], inner_a: 0.85, outer: [0.58, 0.38, 1.0], outer_a: 0.0, power: 1.6 },
                ..Layer::new("Trail glow", Shape::Disc { radius: 24.0 })
            }],
            // Official osu! sliderb: 236x236 @2x (118x118 SD), matching the 236 px hitcircle diameter.
            ElementKind::SliderBall => vec![
                Layer {
                    name: "Inner tint".into(),
                    opacity: 0.22,
                    shape: Shape::Disc { radius: 112.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    ..Layer::new("Inner tint", Shape::Disc { radius: 112.0 })
                },
                Layer {
                    name: "Ball ring".into(),
                    shape: Shape::Ring { radius: 112.0, thickness: 10.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    glow: GlowStyle { enabled: true, radius: 6.0, color: [1.0, 1.0, 1.0], intensity: 0.35, inner: false },
                    ..Layer::new("Ball ring", Shape::Ring { radius: 112.0, thickness: 10.0 })
                },
            ],
            // Official osu! sliderfollowcircle: 512x512 @2x (256x256 SD).
            ElementKind::SliderFollowCircle => vec![
                Layer {
                    name: "Follow tint".into(),
                    opacity: 0.10,
                    shape: Shape::Disc { radius: 232.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                    ..Layer::new("Follow tint", Shape::Disc { radius: 232.0 })
                },
                Layer {
                    name: "Follow ring".into(),
                    shape: Shape::Ring { radius: 232.0, thickness: 8.0 },
                    fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 0.92 },
                    glow: GlowStyle { enabled: true, radius: 14.0, color: [1.0, 1.0, 1.0], intensity: 0.35, inner: false },
                    ..Layer::new("Follow ring", Shape::Ring { radius: 232.0, thickness: 8.0 })
                },
            ],
            ElementKind::ReverseArrow => vec![Layer {
                name: "Chevron".into(),
                shape: Shape::Chevron { size: 52.0, thickness: 14.0, spread: 44.0 },
                fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                glow: GlowStyle { enabled: true, radius: 14.0, color: [1.0, 1.0, 1.0], intensity: 0.45, inner: false },
                ..Layer::new("Chevron", Shape::Chevron { size: 52.0, thickness: 14.0, spread: 44.0 })
            }],
            // Official osu! sliderscorepoint: 32x32 @2x (16x16 SD).
            ElementKind::SliderTick => vec![Layer {
                name: "Tick dot".into(),
                shape: Shape::Disc { radius: 9.0 },
                fill: FillStyle::Solid { color: [1.0, 1.0, 1.0], alpha: 1.0 },
                glow: GlowStyle { enabled: true, radius: 5.0, color: [1.0, 1.0, 1.0], intensity: 0.55, inner: false },
                ..Layer::new("Tick dot", Shape::Disc { radius: 9.0 })
            }],
            ElementKind::Custom => vec![Layer::new("Disc", Shape::Disc { radius: 118.0 })],
        };
        ElementDoc { kind, name, width, height, layers }
    }

    /// Render the full layer stack into a 32-bit premultiplied float `Img` in parallel.
    pub fn render(&self) -> Img {
        let w = self.width.clamp(2, 2048);
        let h = self.height.clamp(2, 2048);
        let cx = w as f32 * 0.5;
        let cy = h as f32 * 0.5;
        let active: Vec<&Layer> = self.layers.iter().filter(|l| l.visible && l.opacity > 1e-4).collect();
        let mut px = vec![[0.0f32; 4]; w * h];
        px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let py0 = y as f32 + 0.5 - cy;
            for (x, out) in row.iter_mut().enumerate() {
                let px0 = x as f32 + 0.5 - cx;
                let mut acc = [0.0f32; 4];
                for layer in &active {
                    let s = eval_layer(layer, px0, py0);
                    if s[3] > 1e-6 || s[0] > 1e-6 || s[1] > 1e-6 || s[2] > 1e-6 {
                        acc = blend(acc, s, layer.blend);
                    }
                }
                *out = acc;
            }
        });
        Img { w, h, px }
    }
}

fn eval_layer(l: &Layer, px: f32, py: f32) -> [f32; 4] {
    let tx = px - l.offset[0];
    let ty = py - l.offset[1];
    let rad = -l.rotation.to_radians();
    let (sn, cs) = rad.sin_cos();
    let x = tx * cs - ty * sn;
    let y = tx * sn + ty * cs;

    let (d, extent) = shape_sdf(l.shape, x, y);
    let aa = (0.75 + l.feather.max(0.0)).max(0.5);
    let fill_cov = smooth_cov(-d, aa);

    let mut cur = [0.0f32; 4];

    // 1. Outer glow (behind fill)
    if l.glow.enabled && !l.glow.inner && l.glow.radius > 0.1 && l.glow.intensity > 1e-4 {
        let dist = d.max(0.0);
        let r = l.glow.radius.max(0.5);
        // Dual-lobe smooth falloff (Gaussian core + soft exponential tail) for natural osu! glow with zero banding
        let t = dist / r;
        let g = (0.65 * (-2.8 * t * t).exp() + 0.35 * (-2.2 * t).exp()) * (1.0 - (t / 3.0).clamp(0.0, 1.0)).powi(2);
        let ga = (g * l.glow.intensity).clamp(0.0, 1.0);
        if ga > 1e-5 {
            cur = premult_over(cur, [l.glow.color[0] * ga, l.glow.color[1] * ga, l.glow.color[2] * ga, ga]);
        }
    }

    // 2. Shape fill
    if fill_cov > 1e-5 {
        let (fc, fa) = eval_fill(l.fill, x, y, extent);
        let a = (fill_cov * fa).clamp(0.0, 1.0);
        cur = premult_over(cur, [fc[0] * a, fc[1] * a, fc[2] * a, a]);
    }

    // 3. Inner glow (clipped inside shape)
    if l.glow.enabled && l.glow.inner && fill_cov > 1e-5 && l.glow.radius > 0.1 && l.glow.intensity > 1e-4 {
        let inside_d = (-d).max(0.0);
        let t = inside_d / l.glow.radius.max(0.5);
        let g = (-2.6 * t * t).exp();
        let ga = (g * l.glow.intensity * fill_cov).clamp(0.0, 1.0);
        cur = premult_over(cur, [l.glow.color[0] * ga, l.glow.color[1] * ga, l.glow.color[2] * ga, ga]);
    }

    // 4. Stroke
    if l.stroke.enabled && l.stroke.width > 0.05 && l.stroke.opacity > 1e-4 {
        let w = l.stroke.width;
        let sd = match l.stroke.align {
            StrokeAlign::Inside => (d + w * 0.5).abs() - w * 0.5,
            StrokeAlign::Center => d.abs() - w * 0.5,
            StrokeAlign::Outside => (d - w * 0.5).abs() - w * 0.5,
        };
        let scov = smooth_cov(-sd, 0.75);
        if scov > 1e-5 {
            let sa = (scov * l.stroke.opacity).clamp(0.0, 1.0);
            cur = premult_over(cur, [l.stroke.color[0] * sa, l.stroke.color[1] * sa, l.stroke.color[2] * sa, sa]);
        }
    }

    let op = l.opacity.clamp(0.0, 1.0);
    [cur[0] * op, cur[1] * op, cur[2] * op, cur[3] * op]
}

fn smooth_cov(signed_inside: f32, half_width: f32) -> f32 {
    let t = ((signed_inside + half_width) / (2.0 * half_width)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Returns `(signed_distance_px, characteristic_radius)` where `< 0` is inside the shape.
fn shape_sdf(shape: Shape, x: f32, y: f32) -> (f32, f32) {
    match shape {
        Shape::Disc { radius } => {
            let r = radius.max(0.5);
            ((x * x + y * y).sqrt() - r, r)
        }
        Shape::Ring { radius, thickness } => {
            let r = radius.max(0.5);
            let h = (thickness * 0.5).max(0.25);
            (((x * x + y * y).sqrt() - r).abs() - h, r + h)
        }
        Shape::RoundedRect { width, height, corner } => {
            let hw = (width * 0.5).max(0.5);
            let hh = (height * 0.5).max(0.5);
            let cr = corner.clamp(0.0, hw.min(hh));
            let qx = x.abs() - (hw - cr);
            let qy = y.abs() - (hh - cr);
            let out = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let ins = qx.max(qy).min(0.0);
            (out + ins - cr, hw.max(hh))
        }
        Shape::Polygon { sides, radius, corner } => {
            let n = sides.clamp(3, 12) as f32;
            let r = radius.max(1.0);
            let cr = corner.clamp(0.0, r * 0.6);
            let an = std::f32::consts::PI / n;
            let he = (r - cr) * an.cos();
            let mut a = y.atan2(x) + std::f32::consts::FRAC_PI_2;
            let sector = 2.0 * an;
            a = (a.rem_euclid(sector)) - an;
            let len = (x * x + y * y).sqrt();
            let px = len * a.cos();
            let py = len * a.sin().abs();
            let edge_y = (r - cr) * an.sin();
            let dx = px - he;
            let dy = (py - edge_y).max(0.0);
            let d = if dx > 0.0 { (dx * dx + dy * dy).sqrt() } else { dx.max((py - edge_y).min(0.0)) };
            (d - cr, r)
        }
        Shape::Chevron { size, thickness, spread } => {
            let sz = size.max(2.0);
            let th = (thickness * 0.5).max(0.5);
            let sp = spread.max(2.0);
            // Right-pointing chevron centered around origin: two capsules meeting at (+sz*0.4, 0)
            let tip = (sz * 0.4, 0.0);
            let arm_x = -sz * 0.4;
            let d1 = sd_segment(x, y, arm_x, -sp, tip.0, tip.1) - th;
            let d2 = sd_segment(x, y, arm_x, sp, tip.0, tip.1) - th;
            (d1.min(d2), sz.max(sp))
        }
        Shape::Crosshair { length, thickness, gap } => {
            let len = length.max(1.0);
            let th = (thickness * 0.5).max(0.5);
            let g = gap.max(0.0);
            let dh = sd_segment(x.abs(), y, g, 0.0, g + len, 0.0) - th;
            let dv = sd_segment(x, y.abs(), 0.0, g, 0.0, g + len) - th;
            (dh.min(dv), g + len)
        }
    }
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let denom = (bax * bax + bay * bay).max(1e-6);
    let h = ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0);
    ((pax - bax * h).powi(2) + (pay - bay * h).powi(2)).sqrt()
}

fn eval_fill(fill: FillStyle, x: f32, y: f32, extent: f32) -> ([f32; 3], f32) {
    match fill {
        FillStyle::Solid { color, alpha } => (color, alpha.clamp(0.0, 1.0)),
        FillStyle::Radial { inner, inner_a, outer, outer_a, power } => {
            let t = ((x * x + y * y).sqrt() / extent.max(1.0)).clamp(0.0, 1.0).powf(power.clamp(0.15, 6.0));
            (
                [inner[0] + (outer[0] - inner[0]) * t, inner[1] + (outer[1] - inner[1]) * t, inner[2] + (outer[2] - inner[2]) * t],
                (inner_a + (outer_a - inner_a) * t).clamp(0.0, 1.0),
            )
        }
        FillStyle::Linear { top, top_a, bottom, bottom_a, angle } => {
            let rad = angle.to_radians();
            let proj = x * rad.cos() + y * rad.sin();
            let t = (proj / (2.0 * extent.max(1.0)) + 0.5).clamp(0.0, 1.0);
            (
                [top[0] + (bottom[0] - top[0]) * t, top[1] + (bottom[1] - top[1]) * t, top[2] + (bottom[2] - top[2]) * t],
                (top_a + (bottom_a - top_a) * t).clamp(0.0, 1.0),
            )
        }
    }
}

fn premult_over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let inv = 1.0 - src[3];
    [
        (src[0] + dst[0] * inv).clamp(0.0, 1.0),
        (src[1] + dst[1] * inv).clamp(0.0, 1.0),
        (src[2] + dst[2] * inv).clamp(0.0, 1.0),
        (src[3] + dst[3] * inv).clamp(0.0, 1.0),
    ]
}

fn blend(dst: [f32; 4], src: [f32; 4], mode: BlendMode) -> [f32; 4] {
    match mode {
        BlendMode::Normal => premult_over(dst, src),
        BlendMode::Add => {
            let a = (dst[3] + src[3]).clamp(0.0, 1.0);
            [(dst[0] + src[0]).min(a), (dst[1] + src[1]).min(a), (dst[2] + src[2]).min(a), a]
        }
        BlendMode::Screen => {
            let a = (dst[3] + src[3] - dst[3] * src[3]).clamp(0.0, 1.0);
            let ch = |d: f32, s: f32| (d + s - d * s).clamp(0.0, a);
            [ch(dst[0], src[0]), ch(dst[1], src[1]), ch(dst[2], src[2]), a]
        }
        BlendMode::Multiply => {
            let inv_s = 1.0 - src[3];
            let inv_d = 1.0 - dst[3];
            let a = (src[3] + dst[3] * inv_s).clamp(0.0, 1.0);
            let ch = |d: f32, s: f32| (d * s + d * inv_s + s * inv_d).clamp(0.0, a);
            [ch(dst[0], src[0]), ch(dst[1], src[1]), ch(dst[2], src[2]), a]
        }
        BlendMode::Erase => {
            let keep = (1.0 - src[3]).clamp(0.0, 1.0);
            [dst[0] * keep, dst[1] * keep, dst[2] * keep, dst[3] * keep]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_renders_valid_premultiplied_pixels() {
        for kind in ElementKind::ALL {
            let doc = ElementDoc::template(kind);
            let img = doc.render();
            assert_eq!((img.w, img.h), (doc.width, doc.height));
            let mut has_visible = false;
            for p in &img.px {
                assert!(p[3] >= 0.0 && p[3] <= 1.0);
                for k in 0..3 {
                    assert!(p[k] >= 0.0 && p[k] <= p[3] + 1e-4, "{kind:?}: rgb {} > a {}", p[k], p[3]);
                }
                if p[3] > 0.2 {
                    has_visible = true;
                }
            }
            assert!(has_visible, "{kind:?} template was completely blank");
        }
    }

    #[test]
    fn ring_has_hollow_center_and_erase_punches_hole() {
        let mut doc = ElementDoc {
            kind: ElementKind::Custom,
            name: "test".into(),
            width: 64,
            height: 64,
            layers: vec![
                Layer::new("Disc", Shape::Disc { radius: 24.0 }),
                Layer { blend: BlendMode::Erase, ..Layer::new("Hole", Shape::Disc { radius: 10.0 }) },
            ],
        };
        let img = doc.render();
        let center = img.px[32 * 64 + 32];
        let rim = img.px[32 * 64 + 48];
        assert!(center[3] < 0.01, "erased center should be transparent: {center:?}");
        assert!(rim[3] > 0.95, "outer rim should stay solid: {rim:?}");
        // Round-trip serde JSON
        let json = serde_json::to_string(&doc).unwrap();
        let back: ElementDoc = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
        doc.layers.clear();
    }
}
