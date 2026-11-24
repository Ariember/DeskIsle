//! CPU port of PrismGlass Desktop's classic Liquid Glass material (`glass-renderer.js`,
//! preset "I", the default `styleEnabled = 0` path) for a resting surface: no pointer,
//! hover, press or Astro effects. Coordinates are physical pixels, like the original.
//!
//! PrismGlass Desktop: MIT, Copyright (c) 2026 liburce0412-alt and PrismGlass
//! contributors. Its circular thickness/bevel and Snell/Fresnel model is adapted from
//! webgl-apple-liquid-glass: MIT, Copyright (c) 2026 Oliver Nemo.

/// A BGRA image the glass refracts. Alpha is ignored, as GDI leaves it undefined.
pub(super) struct Scene<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [u8],
    /// Scene pixels per output pixel, for samples taken at a reduced size.
    pub scale_x: f32,
    pub scale_y: f32,
    /// Output pixel `(0, 0)` lies at this position in output-pixel units of the scene.
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Scene<'_> {
    /// Filter before sampling any transmitted or reflected ray, including those
    /// at the bevel. Decimate after filtering to avoid aliasing narrow stripes.
    fn softened<'a>(&'a self, sigma: f32, pixels: &'a mut Vec<u8>) -> Scene<'a> {
        if sigma == 0. {
            return Scene { ..*self };
        }
        let mut horizontal = Vec::new();
        let (width, step_x) = blur_rows(
            self.pixels,
            self.width,
            self.height,
            sigma * self.scale_x,
            &mut horizontal,
        );
        let (height, step_y) = blur_rows(
            &horizontal,
            self.height,
            width,
            sigma * self.scale_y,
            pixels,
        );
        Scene {
            width,
            height,
            pixels,
            scale_x: self.scale_x / step_x as f32,
            scale_y: self.scale_y / step_y as f32,
            ..*self
        }
    }

    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && (self.width as usize)
                .checked_mul(self.height as usize)
                .and_then(|n| n.checked_mul(4))
                == Some(self.pixels.len())
    }

    fn texel(&self, x: i32, y: i32) -> [f32; 3] {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        let i = (y * self.width as usize + x) * 4;
        let p = &self.pixels[i..i + 3];
        [
            f32::from(p[2]) / 255.,
            f32::from(p[1]) / 255.,
            f32::from(p[0]) / 255.,
        ]
    }

    /// Bilinear, edge-clamped sample at output-pixel position `(x, y)`, matching WebGL's
    /// `LINEAR` filter with `CLAMP_TO_EDGE`.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let u = (x + self.offset_x) * self.scale_x - 0.5;
        let v = (y + self.offset_y) * self.scale_y - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// One separable Gaussian pass. Store the result transposed so the same row
/// filter can run vertically; the second pass restores the original orientation.
fn blur_rows(src: &[u8], width: u32, height: u32, sigma: f32, dest: &mut Vec<u8>) -> (u32, u32) {
    let step = sigma.floor().max(1.) as u32;
    let columns = width.div_ceil(step);
    // Match Scene::sample's half-texel convention on the reduced grid.
    let center = (step - 1) as f32 * 0.5;
    let first = (center - 3. * sigma).ceil() as i32;
    let last = (center + 3. * sigma).floor() as i32;
    let weights: Vec<f32> = (first..=last)
        .map(|i| (-0.5 * ((i as f32 - center) / sigma).powi(2)).exp())
        .collect();
    let total: f32 = weights.iter().sum();
    dest.resize(columns as usize * height as usize * 4, 255);
    for (y, row) in src
        .as_chunks::<4>()
        .0
        .chunks_exact(width as usize)
        .enumerate()
    {
        for x in 0..columns {
            let start = (x * step) as i32 + first;
            let mut sum = [0.; 3];
            for (i, weight) in weights.iter().enumerate() {
                let sample = row[(start + i as i32).clamp(0, width as i32 - 1) as usize];
                for (channel, v) in sum.iter_mut().zip(sample) {
                    *channel += weight * f32::from(v);
                }
            }
            let offset = (x as usize * height as usize + y) * 4;
            dest[offset..offset + 3].copy_from_slice(&sum.map(|v| (v / total).round() as u8));
        }
    }
    (columns, step)
}

/// One glass card in output pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Card {
    pub center_x: f32,
    pub center_y: f32,
    pub half_w: f32,
    pub half_h: f32,
    pub radius: f32,
    /// Half height the bevel width derives from; differs from `half_h` only when the
    /// card extends past a screen edge.
    pub bevel_half_h: f32,
    /// PrismGlass `rimStrength`: Dock 1, top island 0.45, labels 0.32.
    pub rim: f32,
    /// PrismGlass `opaqueSurface`: antialiased edges blend with the unrefracted scene.
    pub opaque: bool,
}

impl Card {
    /// A card whose visible glass fills `width` x `height` at `(x, y)` exactly. The
    /// shader insets every card by 2 px, so the native-popup convention grows it by 2.
    pub fn filling(x: f32, y: f32, width: f32, height: f32, radius: f32, rim: f32) -> Self {
        Self {
            center_x: x + width * 0.5,
            center_y: y + height * 0.5,
            half_w: width * 0.5 + 2.,
            half_h: height * 0.5 + 2.,
            radius,
            bevel_half_h: height * 0.5,
            rim,
            opaque: false,
        }
    }
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// How the scene behind a glass body reads, so content on it can pick the ink that
/// stands out most and the body can frost a busy scene a little more.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tone {
    /// Mean WCAG relative luminance (`0..=1`).
    pub luminance: f32,
    /// Standard deviation of gamma-encoded luma (`0..=0.5`): high where detail such as
    /// text or sharp edges behind the glass would compete with its content.
    pub busy: f32,
}

impl Tone {
    /// The scene under `width` x `height` output pixels, read on an even grid of
    /// samples. `None` when the scene holds no pixels.
    pub fn measure(scene: &Scene, width: u32, height: u32) -> Option<Self> {
        if !scene.valid() || width == 0 || height == 0 {
            return None;
        }
        const COLUMNS: u32 = 24;
        const ROWS: u32 = 12;
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (mut light, mut sum, mut squares) = (0f32, 0f32, 0f32);
        for row in 0..ROWS {
            let y = (row as f32 + 0.5) * height as f32 / ROWS as f32;
            for column in 0..COLUMNS {
                let x = (column as f32 + 0.5) * width as f32 / COLUMNS as f32;
                let c = scene.sample(x, y);
                light += dot3(c.map(linear), LUMA);
                let luma = dot3(c, LUMA);
                sum += luma;
                squares += luma * luma;
            }
        }
        let n = (COLUMNS * ROWS) as f32;
        let mean = sum / n;
        Some(Self {
            luminance: light / n,
            busy: (squares / n - mean * mean).max(0.).sqrt(),
        })
    }

    /// Whether dark ink reads better than light ink. Without a `previous` choice this is
    /// the luminance where black and white text have equal WCAG contrast; otherwise
    /// the scene must cross well past it, so a background near the crossover does not
    /// make the content flip at every sample.
    pub fn light_ground(self, previous: Option<bool>) -> bool {
        let crossover = match previous {
            Some(true) => 0.12,
            Some(false) => 0.26,
            None => 0.179,
        };
        self.luminance > crossover
    }

    /// Busy scenes fade out the remaining sharp detail faster. Both slider
    /// endpoints still give exactly clear or fully system-blurred transmission.
    pub fn frost(self, base: f32) -> f32 {
        base + base * (1. - base) * smoothstep(0.10, 0.24, self.busy)
    }
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = dot3(v, v).sqrt();
    if l > 0. { v.map(|c| c / l) } else { v }
}

/// GLSL `refract(I, N, eta)` for `I = (0, 0, -1)`.
fn refract_down(n: [f32; 3], eta: f32) -> [f32; 3] {
    let i = [0., 0., -1.];
    let d = dot3(n, i);
    let k = 1. - eta * eta * (1. - d * d);
    if k < 0. {
        return [0.; 3];
    }
    let s = eta * d + k.sqrt();
    std::array::from_fn(|c| eta * i[c] - s * n[c])
}

/// Signed distance to a rounded rectangle with half extents `b`.
fn sd(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let r = r.clamp(0., bx.min(by).max(0.));
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    let ox = qx.max(0.);
    let oy = qy.max(0.);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.) - r
}

/// Renders `card` over `scene` into a premultiplied BGRA image of `width` x `height`.
pub(super) fn render(scene: &Scene, width: u32, height: u32, card: Card, blur_px: f32) -> Vec<u8> {
    render_card(scene, width, height, card, blur_px, false)
}

/// Renders the curved glass edge over a live desktop. The flat body is transparent
/// so changes in the windows underneath show through between bevel samples.
pub(super) fn render_bevel(
    scene: &Scene,
    width: u32,
    height: u32,
    card: Card,
    blur_px: f32,
) -> Vec<u8> {
    render_card(scene, width, height, card, blur_px, true)
}

fn render_card(
    scene: &Scene,
    width: u32,
    height: u32,
    card: Card,
    blur_px: f32,
    bevel_only: bool,
) -> Vec<u8> {
    let mut out = vec![0u8; width as usize * height as usize * 4];
    if !scene.valid() || width == 0 || height == 0 {
        return out;
    }
    let rows = height as usize;
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8)
        .min(rows);
    let band = rows.div_ceil(threads);
    let mut samples = Vec::new();
    let filtered = scene.softened(blur_px, &mut samples);
    let scene = &filtered;
    let ambient = scene.sample(card.center_x, card.center_y);
    std::thread::scope(|s| {
        for (index, chunk) in out.chunks_mut(band * width as usize * 4).enumerate() {
            s.spawn(move || {
                for (row, line) in chunk.chunks_exact_mut(width as usize * 4).enumerate() {
                    let y = (index * band + row) as u32;
                    for (x, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let [r, g, b, a] = shade(
                            scene,
                            card,
                            ambient,
                            x as f32 + 0.5,
                            y as f32 + 0.5,
                            bevel_only,
                        );
                        let byte = |v: f32| (v.clamp(0., 1.) * 255. + 0.5) as u8;
                        px.copy_from_slice(&[byte(b), byte(g), byte(r), byte(a)]);
                    }
                }
            });
        }
    });
    out
}

/// The fragment shader for one pixel centre `(px, py)`; returns premultiplied RGBA.
fn shade(
    scene: &Scene,
    card: Card,
    ambient: [f32; 3],
    px: f32,
    py: f32,
    bevel_only: bool,
) -> [f32; 4] {
    let (dx, dy) = (px - card.center_x, py - card.center_y);
    let bx = (card.half_w - 2.).max(0.01);
    let by = (card.half_h - 2.).max(0.01);
    let r = card.radius;
    let dist = sd(dx, dy, bx, by, r);
    let depth = (-dist).max(0.);
    let bevel = (card.bevel_half_h * 0.46).clamp(10., 30.);
    if dist > 8. || (bevel_only && (depth >= bevel || dist >= 0.8)) {
        return [0.; 4];
    }
    let mut coverage = 1. - smoothstep(-0.8, 0.8, dist);
    if bevel_only {
        // Keep the highlight and chromatic fringe at full strength, then blend into
        // the live scene as the surface flattens.
        coverage *= 1. - smoothstep(3., bevel, depth);
    }
    let mut result = [0.; 3];
    if dist < 1. {
        let e = 0.7;
        let gx = sd(dx + e, dy, bx, by, r) - sd(dx - e, dy, bx, by, r) + 0.0001;
        let gy = sd(dx, dy + e, bx, by, r) - sd(dx, dy - e, bx, by, r) + 0.0001;
        let gl = (gx * gx + gy * gy).sqrt().max(1e-12);
        let (gx, gy) = (gx / gl, gy / gl);

        let rim = (-depth / bevel).exp();
        let t = (depth / bevel).clamp(0., 1.);
        let arc = (1. - (1. - t) * (1. - t)).max(0.).sqrt();
        let slope = 0.72 * (1. - t) / arc.max(0.16);
        let normal = normalize3([-gx * slope, -gy * slope, 1.]);

        let optical_path = bevel * (0.8 + 1.15 * arc);
        let bend = |eta: f32| {
            let ray = refract_down(normal, eta);
            let z = (-ray[2]).max(0.3);
            (
                ray[0] / z * optical_path + dx * 0.008,
                ray[1] / z * optical_path + dy * 0.008,
            )
        };
        let strength = mix(0.78, 1., card.rim);
        let (rx, ry) = bend(1. / (1.46 - 0.065));
        let (gx_, gy_) = bend(1. / 1.46);
        let (bx_, by_) = bend(1. / (1.46 + 0.065));
        let (ax, ay) = (px + gx_ * strength, py + gy_ * strength);
        let mut c = [
            scene.sample(px + rx * strength, py + ry * strength)[0],
            scene.sample(ax, ay)[1],
            scene.sample(px + bx_ * strength, py + by_ * strength)[2],
        ];

        // The bevel diffuses bent rays as it flattens into the body.
        if t < 1. {
            let f = mix(2.2, 0.65, rim);
            let mut diffusion = scene.sample(ax, ay).map(|v| v * 4.);
            for (ox, oy, w) in [
                (f, 0., 2.),
                (-f, 0., 2.),
                (0., f, 2.),
                (0., -f, 2.),
                (f, f, 1.),
                (-f, -f, 1.),
                (f, -f, 1.),
                (-f, f, 1.),
            ] {
                let s = scene.sample(ax + ox, ay + oy);
                for k in 0..3 {
                    diffusion[k] += s[k] * w;
                }
            }
            let amount = mix(0.5, 0.18, rim) * (1. - smoothstep(0.6, 1., t));
            c = mix3(c, diffusion.map(|v| v / 16.), amount);
        }

        let luminance = dot3(c, LUMA);
        c = mix3([luminance; 3], c, 1.16);
        let bright = smoothstep(0.35, 0.88, luminance);
        let dark = 1. - smoothstep(0.04, 0.3, luminance);
        // Neutral backgrounds pick up a little surrounding color instead of a gray veil.
        let chroma = c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
        let ambient_luma = dot3(ambient, LUMA).max(0.12);
        let tinted = ambient.map(|v| v * luminance / ambient_luma);
        c = mix3(c, tinted, (1. - smoothstep(0.05, 0.22, chroma)) * 0.08);
        let clear = mix(1., 0.86, bright);
        c = c.map(|v| v * clear);
        for (k, lift) in [0.012, 0.018, 0.027].into_iter().enumerate() {
            c[k] += lift * dark;
        }
        let fresnel = 0.04 + 0.96 * (1. - normal[2]).powi(5);
        let reflection = scene.sample(px + gx * bevel * 2., py + gy * bevel * 2.);
        let mirror = [0.28, 0.32, 0.36];
        let reflected: [f32; 3] = std::array::from_fn(|k| reflection[k] * 0.65 + mirror[k]);
        c = mix3(c, reflected, fresnel * 0.38 * rim);

        let (lx, ly) = (-0.65f32, -0.85f32);
        let ll = (lx * lx + ly * ly).sqrt();
        let light = (gx * lx / ll + gy * ly / ll).max(0.).powi(4);
        let specular = dot3(normal, normalize3([0.4, 0.6, 1.])).max(0.).powi(32);
        let band = (-((depth - 0.9) / 0.48).powi(2)).exp();
        let lift = band * (0.10 + 0.44 * light) * (1. - bright * 0.35) * mix(0.5, 1., card.rim)
            + specular * rim * 0.16
            - rim * (1. - light) * 0.045;
        result = c.map(|v| v + lift);
    }
    if bevel_only {
        return bevel_overlay(scene.sample(px, py), result, coverage);
    }
    if card.opaque {
        result = mix3(scene.sample(px, py), result, coverage);
        coverage = 1.;
    }
    // WebGL clamps the color on write; the premultiplied output keeps that order.
    let result = result.map(|v| v.clamp(0., 1.) * coverage);
    [result[0], result[1], result[2], coverage]
}

/// Encodes just the refracted color change in a premultiplied overlay. For each channel,
/// `source + (1 - alpha) * background = target` with `0 <= source <= alpha`.
/// The tightest channel sets the least opacity that can reproduce the sampled glass.
fn bevel_overlay(background: [f32; 3], target: [f32; 3], coverage: f32) -> [f32; 4] {
    let target = target.map(|v| v.clamp(0., 1.));
    let mut alpha = 0f32;
    for (base, desired) in background.into_iter().zip(target) {
        if desired > base {
            alpha = alpha.max((desired - base) / (1. - base));
        } else if desired < base {
            alpha = alpha.max((base - desired) / base);
        }
    }
    let [r, g, b] = std::array::from_fn(|i| {
        (target[i] - (1. - alpha) * background[i]).clamp(0., alpha) * coverage
    });
    [r, g, b, alpha * coverage]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                // Blue on the left half, red on the right half.
                if x < w / 2 {
                    pixels.extend_from_slice(&[230, 90, 30, 0]);
                } else {
                    pixels.extend_from_slice(&[30, 70, 230, 0]);
                }
                let _ = y;
            }
        }
        pixels
    }

    fn uniform(pixel: [u8; 4], w: u32, h: u32) -> Vec<u8> {
        pixel.repeat((w * h) as usize)
    }

    fn scene_of(pixels: &[u8], w: u32, h: u32) -> Scene<'_> {
        Scene {
            width: w,
            height: h,
            pixels,
            scale_x: 1.,
            scale_y: 1.,
            offset_x: 0.,
            offset_y: 0.,
        }
    }

    #[test]
    fn tone_picks_the_ink_with_more_contrast_and_holds_it_near_the_crossover() {
        let (w, h) = (96, 32);
        let light = uniform([235, 240, 245, 0], w, h);
        let dark = uniform([40, 30, 20, 0], w, h);
        let tone = |p: &[u8]| Tone::measure(&scene_of(p, w, h), w, h).unwrap();
        assert!(tone(&light).light_ground(None));
        assert!(!tone(&dark).light_ground(None));
        // Saturated yellow is light enough for black text; saturated blue is not.
        assert!(tone(&uniform([0, 220, 255, 0], w, h)).light_ground(None));
        assert!(!tone(&uniform([255, 60, 0, 0], w, h)).light_ground(None));
        // Mid gray (sRGB 118, luminance ~0.18) keeps whichever ink was already chosen.
        let gray = tone(&uniform([118, 118, 118, 0], w, h));
        assert!(gray.light_ground(Some(true)));
        assert!(!gray.light_ground(Some(false)));
    }

    #[test]
    fn busy_scenes_frost_more_than_flat_ones() {
        let (w, h) = (96, 32);
        let flat = uniform([90, 120, 150, 0], w, h);
        // Black and white stripes, as text or a web page behind the glass would give.
        let mut stripes = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let v = if x % 7 < 3 { 0 } else { 255 };
                stripes.extend_from_slice(&[v, v, v, 0]);
            }
        }
        let flat = Tone::measure(&scene_of(&flat, w, h), w, h).unwrap();
        let busy = Tone::measure(&scene_of(&stripes, w, h), w, h).unwrap();
        assert!((flat.frost(0.3) - 0.3).abs() < 1e-3);
        assert!(busy.frost(0.3) > 0.5);
        assert!(busy.frost(0.8) > 0.9);
        assert_eq!(busy.frost(0.), 0.);
        assert_eq!(busy.frost(1.), 1.);
    }

    #[test]
    fn blur_softens_stripes_across_the_body_straight_edges_and_rounded_corners() {
        let (w, h) = (137, 73);
        // Include the wallpaper outside the card that the curved bevel refracts.
        let pad = 64;
        let (sw, sh) = (w + 2 * pad, h + 2 * pad);
        let stripes: Vec<_> = (0..sw * sh)
            .flat_map(|i| {
                let v = if (i % sw + i / sw) % 6 < 3 { 48 } else { 208 };
                [v, v, v, 255]
            })
            .collect();
        let flat = uniform([128, 128, 128, 255], sw, sh);
        let card = Card::filling(0., 0., w as f32, h as f32, 18., 1.);
        for draw in [render, render_bevel] {
            let paint = |pixels| {
                let scene = Scene {
                    offset_x: pad as f32,
                    offset_y: pad as f32,
                    ..scene_of(pixels, sw, sh)
                };
                draw(&scene, w, h, card, 12.8)
            };
            let soft = paint(&stripes);
            let reference = paint(&flat);
            let over_gray = |image: &[u8], x, y, channel: usize| {
                let i = ((y * w + x) * 4) as usize;
                f32::from(image[i + channel]) + 128. * (1. - f32::from(image[i + 3]) / 255.)
            };
            // A system-blurred background for these stripes is midgray. The foreground
            // bevel must retain its highlights without reintroducing the sharp stripes.
            for (x0, y, width) in [(48, 36, 36), (48, 3, 36), (3, 32, 12), (6, 7, 11)] {
                for x in x0..x0 + width {
                    for channel in 0..3 {
                        let delta =
                            over_gray(&soft, x, y, channel) - over_gray(&reference, x, y, channel);
                        assert!(
                            delta.abs() < 6.,
                            "at ({x}, {y}), channel {channel}: {delta}"
                        );
                    }
                }
            }
            assert_eq!(soft[..4], [0; 4]);
            assert!(over_gray(&soft, 40, 1, 0) > over_gray(&soft, 40, 20, 0) + 4.);
        }
    }

    #[test]
    fn glass_fills_its_shape_keeps_scene_colors_and_lights_the_top_rim() {
        let (w, h) = (160u32, 60u32);
        let pixels = checker(w, h);
        let scene = Scene {
            width: w,
            height: h,
            pixels: &pixels,
            scale_x: 1.,
            scale_y: 1.,
            offset_x: 0.,
            offset_y: 0.,
        };
        let card = Card::filling(0., 0., w as f32, h as f32, 18., 1.);
        let out = render(&scene, w, h, card, 16.);
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        // Rounded corners stay clear; the body is fully covered.
        assert_eq!(at(0, 0)[3], 0);
        assert_eq!(at(80, 30)[3], 255);
        // Clear glass transmits the hues beneath instead of graying them.
        let left = at(30, 30);
        let right = at(130, 30);
        assert!(left[0] > left[2], "left={left:?}");
        assert!(right[2] > right[0], "right={right:?}");
        // A wider blur spreads the red/blue transition further into each side,
        // while the two broad fields above still transmit their original hues.
        let narrow = render(&scene, w, h, card, 3.);
        let red = |image: &[u8], x| image[((30 * w + x) * 4 + 2) as usize];
        assert!(red(&out, 74) > red(&narrow, 74) + 20);
        assert!(red(&narrow, 86) > red(&out, 86) + 20);
        // The lit top-left rim is brighter than the body just inside it.
        let luma = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        assert!(luma(at(40, 1)) > luma(at(40, 20)));
    }

    #[test]
    fn bevel_refracts_scene_from_beyond_the_edge() {
        // A bright band just outside the left edge shows up in the bevel through
        // refraction, while the flat center shows the dark scene almost unchanged.
        let (w, h) = (120u32, 80u32);
        let (sw, pad) = (w + 80, 40u32);
        let mut pixels = vec![40u8; (sw * h * 4) as usize];
        for y in 0..h {
            for x in pad - 18..pad - 6 {
                let i = ((y * sw + x) * 4) as usize;
                pixels[i..i + 3].copy_from_slice(&[250, 250, 250]);
            }
        }
        let scene = Scene {
            width: sw,
            height: h,
            pixels: &pixels,
            scale_x: 1.,
            scale_y: 1.,
            offset_x: pad as f32,
            offset_y: 0.,
        };
        let card = Card::filling(0., 0., w as f32, h as f32, 20., 1.);
        let out = render(&scene, w, h, card, 0.);
        let red = |x: u32| out[((40 * w + x) * 4 + 2) as usize];
        let brightest = (1..24).map(red).max().unwrap();
        assert!(brightest > 120, "bevel did not refract the band");
        assert!(red(60) < 80, "center={}", red(60));

        let edge = render_bevel(&scene, w, h, card, 0.);
        let pixel = |x: u32| ((40 * w + x) * 4) as usize;
        for x in [1, 10] {
            let i = pixel(x);
            let coverage = 1. - smoothstep(3., 18.4, x as f32 + 0.5);
            let transmission = 1. - f32::from(edge[i + 3]) / 255.;
            for channel in 0..3 {
                let composited = f32::from(edge[i + channel]) + transmission * 40.;
                let expected = mix(40., f32::from(out[i + channel]), coverage);
                assert!((composited - expected).abs() <= 1.5);
                assert!(edge[i + channel] <= edge[i + 3]);
            }
        }
        let center = pixel(60);
        assert_eq!(edge[center..center + 4], [0; 4]);
    }

    #[test]
    fn lit_rim_transmits_an_updated_window_without_resampling() {
        let (w, h) = (48, 32);
        let base = [80, 130, 100, 0];
        let pixels = base.repeat((w * h) as usize);
        let scene = Scene {
            width: w,
            height: h,
            pixels: &pixels,
            scale_x: 1.,
            scale_y: 1.,
            offset_x: 0.,
            offset_y: 0.,
        };
        let card = Card::filling(0., 0., w as f32, h as f32, 8., 1.);
        let prism = render(&scene, w, h, card, 12.8);
        let edge = render_bevel(&scene, w, h, card, 0.);
        let i = ((w + w / 2) * 4) as usize; // Middle of the lit top edge, 1.5 px in.
        let transmission = 1. - f32::from(edge[i + 3]) / 255.;
        for (channel, updated) in [200., 50., 40.].into_iter().enumerate() {
            let over = |background| f32::from(edge[i + channel]) + transmission * background;
            let old = over(f32::from(base[channel]));
            assert!((old - f32::from(prism[i + channel])).abs() <= 1.5);
            assert!((over(updated) - old).abs() > 20.);
        }
    }
}
