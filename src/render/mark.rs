//! The DeskIsle emblem at the leading edge of the status bar, rendered on its own
//! DirectComposition subtree so tactile press, hover, and menu-open geometric morphing
//! run on the compositor without repainting the full-width bar.
//!
//! At rest (`t = 0`), the emblem is a crisp circular ring (`○`). Pressing the left
//! button compresses it inward and pre-tensions its curvature into a tilted squircle;
//! opening the system menu unfolds and rotates the contour `45°` clockwise into a
//! four-pointed concave-faceted compass diamond (`◇`) while a counter-rotating
//! star-diamond isle core (`◆`) crystallizes at its center (`❖`) and a faint halo
//! ripples outward.

use std::{
    f32::consts::{FRAC_PI_4, TAU},
    time::Instant,
};

use windows::Win32::Graphics::{
    Direct2D::{Common::*, *},
    DirectComposition::*,
};
use windows::core::Result;

use super::canvas::{Canvas, Palette, Rgb, paint};
use super::{Renderer, TOP_SHADOW_Y, ease, keyframes};

/// Center of the DeskIsle mark from the status bar's leading edge, in DIPs.
const MARK_X: f32 = 18.;
/// Resting circle radius and stroke width, in DIPs.
const MARK_RADIUS: f32 = 6.;
const MARK_STROKE: f32 = 1.75;
/// Square cell size in DIPs for each vector frame and the click halo; fits within the
/// minimum 24 DIP bar height and the 4..32 DIP hit slot.
const MARK_BOX: f32 = 24.;
/// Pre-rasterized vector morph steps from resting ring (`0`) to open compass isle (`36`).
const MARK_FRAMES: usize = 37;
/// Cubic Bezier segments around `[0, 2π)` per closed harmonic contour.
const LOOP_SEGMENTS: usize = 32;

/// Morph target reached while the left mouse button is held on the mark.
const MARK_PRESS_MORPH: f32 = 0.30;
/// Compositor scales for hover lift and tactile press compression.
const MARK_HOVER_SCALE: f32 = 1.07;
const MARK_PRESS_SCALE: f32 = 0.86;

/// Durations in seconds for each interaction phase.
const HOVER_DURATION: f32 = 0.15;
const PRESS_IN_DURATION: f32 = 0.09;
const PRESS_OUT_DURATION: f32 = 0.16;
const OPEN_DURATION: f32 = 0.26;
const CLOSE_DURATION: f32 = 0.20;
const PULSE_DURATION: f32 = 0.30;

/// Smooth closed four-fold contour `r(θ) = mean + quad * cos(4θ) + oct * cos(8θ)`,
/// rotated clockwise by `angle` radians (`angle = 0` aligns tips with N, E, S, W).
#[derive(Clone, Copy, Debug, PartialEq)]
struct HarmonicLoop {
    mean: f32,
    quad: f32,
    oct: f32,
    angle: f32,
}

impl HarmonicLoop {
    fn radius(&self, theta: f32) -> f32 {
        self.mean + self.quad * (4. * theta).cos() + self.oct * (8. * theta).cos()
    }

    /// Position and `dP/dθ` tangent in DIPs relative to the emblem center (`+y` down).
    fn sample(&self, theta: f32) -> ((f32, f32), (f32, f32)) {
        let r = self.radius(theta);
        let dr = -4. * self.quad * (4. * theta).sin() - 8. * self.oct * (8. * theta).sin();
        let (s, c) = (theta + self.angle).sin_cos();
        ((r * s, -r * c), (dr * s + r * c, -dr * c + r * s))
    }
}

/// Complete vector geometry of the emblem at normalized morph progress `t` in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MarkShape {
    rim: HarmonicLoop,
    rim_stroke: f32,
    /// Inner star-diamond isle and its opacity once condensation begins.
    core: Option<(HarmonicLoop, f32)>,
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

impl MarkShape {
    fn at(t: f32) -> Self {
        let t = t.clamp(0., 1.);
        let s = smoothstep(t);
        // Fast initial faceting lets a held mouse-down already show geometric tension.
        let facet = t * (2. - t);
        let rim = HarmonicLoop {
            mean: MARK_RADIUS - 0.44 * facet,
            quad: 1.18 * facet,
            oct: 0.18 * s,
            angle: -FRAC_PI_4 * (1. - s),
        };
        // Swell slightly through mid-morph so press compression keeps visual ink mass.
        let rim_stroke = MARK_STROKE - 0.10 * s + 0.16 * (4. * t * (1. - t));

        let core = (t > 0.14).then(|| {
            let u = ((t - 0.14) / 0.86).clamp(0., 1.);
            let grow = 1. - (1. - u).powi(2);
            let star = smoothstep(u);
            let shape = HarmonicLoop {
                mean: 1.84 * grow,
                quad: 0.54 * star,
                oct: 0.08 * star,
                // Counter-rotates into cardinal lock against the clockwise outer rim.
                angle: FRAC_PI_4 * (1. - star),
            };
            let alpha = (0.35 + 0.65 * grow).min(1.);
            (shape, alpha)
        });

        Self {
            rim,
            rim_stroke,
            core,
        }
    }
}

fn loop_geometry(
    factory: &ID2D1Factory,
    c: &Canvas,
    cx: f32,
    cy: f32,
    contour: HarmonicLoop,
) -> Result<ID2D1PathGeometry> {
    let step = TAU / LOOP_SEGMENTS as f32;
    let third = step / 3.;
    // `windows_numerics::Vector2` lives in a transitive crate; derive it from D2D types.
    let to_px = |(dx, dy): (f32, f32)| {
        let mut point = D2D1_ELLIPSE::default().point;
        point.X = c.origin.x as f32 + (cx + dx) * c.scale;
        point.Y = c.origin.y as f32 + (cy + dy) * c.scale;
        point
    };
    let (start_p, _) = contour.sample(0.);
    let mut segments = [D2D1_BEZIER_SEGMENT::default(); LOOP_SEGMENTS];
    for (index, segment) in segments.iter_mut().enumerate() {
        let t0 = index as f32 * step;
        let t1 = ((index + 1) % LOOP_SEGMENTS) as f32 * step;
        let (p0, d0) = contour.sample(t0);
        let (p1, d1) = contour.sample(t1);
        *segment = D2D1_BEZIER_SEGMENT {
            point1: to_px((p0.0 + third * d0.0, p0.1 + third * d0.1)),
            point2: to_px((p1.0 - third * d1.0, p1.1 - third * d1.1)),
            point3: to_px(p1),
        };
    }
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(to_px(start_p), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddBeziers(&segments);
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        Ok(geometry)
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_shape(
    factory: &ID2D1Factory,
    stroke_style: &ID2D1StrokeStyle,
    c: &Canvas,
    cx: f32,
    cy: f32,
    shape: MarkShape,
    color: Rgb,
    alpha: f32,
) -> Result<()> {
    let rim = loop_geometry(factory, c, cx, cy, shape.rim)?;
    let rim_brush = c.brush_alpha(color, alpha)?;
    unsafe {
        c.ctx
            .DrawGeometry(&rim, &rim_brush, shape.rim_stroke * c.scale, stroke_style);
        if let Some((core, core_alpha)) = shape.core {
            let gem = loop_geometry(factory, c, cx, cy, core)?;
            let core_brush = c.brush_alpha(color, alpha * core_alpha)?;
            c.ctx.FillGeometry(&gem, &core_brush, None);
        }
    }
    Ok(())
}

/// Mirrors a multi-keyframe Hermite curve sent to [`keyframes`], so an interrupted
/// press, spring overshoot, or settle always resumes from its live compositor value.
struct CurveTrack {
    keys: Vec<(f32, f32)>,
    start: Instant,
}

impl CurveTrack {
    fn new(value: f32) -> Self {
        Self {
            keys: vec![(0., value)],
            start: Instant::now(),
        }
    }

    fn goal(&self) -> f32 {
        self.keys.last().map_or(1., |&(_, v)| v)
    }

    fn value(&self) -> f32 {
        self.sample_at(self.start.elapsed().as_secs_f32())
    }

    fn sample_at(&self, elapsed: f32) -> f32 {
        let Some(&(_, first)) = self.keys.first() else {
            return 1.;
        };
        if elapsed <= 0. || self.keys.len() == 1 {
            return first;
        }
        for pair in self.keys.windows(2) {
            let [(t0, v0), (t1, v1)] = [pair[0], pair[1]];
            if elapsed < t1 {
                let d = (t1 - t0).max(1e-5);
                let u = ((elapsed - t0) / d).clamp(0., 1.);
                return v0 + (v1 - v0) * u * u * (3. - 2. * u);
            }
        }
        self.goal()
    }

    fn play(
        &mut self,
        device: &IDCompositionDesktopDevice,
        keys: Vec<(f32, f32)>,
    ) -> Result<IDCompositionAnimation> {
        let curve = keyframes(device, keys.iter().copied())?;
        self.keys = keys;
        self.start = Instant::now();
        Ok(curve)
    }
}

/// Tracks normalized vector morph progress (`0.0..=1.0`) along an ease-out cubic and
/// emits exact frame-crossing step offsets for the pre-rendered Direct2D strip.
struct MorphTrack {
    from: f32,
    goal: f32,
    start: Instant,
    duration: f32,
}

impl MorphTrack {
    fn new(value: f32) -> Self {
        Self {
            from: value,
            goal: value,
            start: Instant::now(),
            duration: 0.,
        }
    }

    fn value(&self) -> f32 {
        if self.duration <= 0. {
            return self.goal;
        }
        let rem = (1. - self.start.elapsed().as_secs_f32() / self.duration).clamp(0., 1.);
        self.goal + (self.from - self.goal) * rem.powi(3)
    }
}

/// Computes `(timestamp_secs, frame_index)` crossings for an ease-out cubic from `from`
/// to `goal` over `duration` seconds across `MARK_FRAMES` discrete vector frames.
fn step_schedule(from: f32, goal: f32, duration: f32) -> (usize, usize, Vec<(f32, usize)>) {
    let max_idx = (MARK_FRAMES - 1) as f32;
    let start_idx = (from.clamp(0., 1.) * max_idx).round() as usize;
    let end_idx = (goal.clamp(0., 1.) * max_idx).round() as usize;
    if start_idx == end_idx || duration <= 0. || (goal - from).abs() < 1e-4 {
        return (start_idx, end_idx, Vec::new());
    }
    let mut steps = Vec::with_capacity(start_idx.abs_diff(end_idx));
    let mut last_t = 0.0_f32;
    let forward = end_idx > start_idx;
    let mut idx = start_idx;
    while idx != end_idx {
        let next_idx = if forward { idx + 1 } else { idx - 1 };
        let midpoint = (idx as f32 + next_idx as f32) * 0.5 / max_idx;
        let ratio = ((midpoint - goal) / (from - goal)).clamp(0., 1.);
        let mut t = duration * (1. - ratio.cbrt());
        // DirectComposition requires strictly increasing segment start times.
        t = t.max(last_t + 1e-4).min(duration - 1e-4);
        if t > last_t && t < duration {
            steps.push((t, next_idx));
            last_t = t;
        }
        idx = next_idx;
    }
    (start_idx, end_idx, steps)
}

pub(super) struct MarkLayer {
    visual: IDCompositionVisual2,
    strip: IDCompositionVisual2,
    scale: IDCompositionScaleTransform,
    pulse_scale: IDCompositionScaleTransform,
    pulse_fade: IDCompositionEffectGroup,
    edge: i32,
    morph: MorphTrack,
    size: CurveTrack,
    hovered: bool,
    pressed: bool,
    open: bool,
    /// Bar height in pixels, DPI, resolved top-bar palette, and drop-shadow state.
    key: (i32, u32, Palette, bool),
}

impl MarkLayer {
    fn rest_scale(&self) -> f32 {
        if self.hovered { MARK_HOVER_SCALE } else { 1. }
    }

    fn apply_morph(
        &mut self,
        device: &IDCompositionDesktopDevice,
        goal: f32,
        duration: f32,
    ) -> Result<bool> {
        if (goal - self.morph.goal).abs() < 1e-3 {
            return Ok(false);
        }
        let from = self.morph.value();
        self.morph = MorphTrack {
            from,
            goal,
            start: Instant::now(),
            duration,
        };
        let (start_idx, end_idx, steps) = step_schedule(from, goal, duration);
        let offset = |idx: usize| -((idx as i32 * self.edge) as f32);
        unsafe {
            if steps.is_empty() {
                self.strip.SetOffsetX2(offset(end_idx))?;
            } else {
                let curve = device.CreateAnimation()?;
                curve.AddCubic(0., offset(start_idx), 0., 0., 0.)?;
                for (t, idx) in steps {
                    curve.AddCubic(t as f64, offset(idx), 0., 0., 0.)?;
                }
                curve.End(duration as f64, offset(end_idx))?;
                self.strip.SetOffsetX(&curve)?;
            }
        }
        Ok(true)
    }

    fn apply_scale(
        &mut self,
        device: &IDCompositionDesktopDevice,
        keys: Vec<(f32, f32)>,
    ) -> Result<()> {
        let curve = self.size.play(device, keys)?;
        unsafe {
            self.scale.SetScaleX(&curve)?;
            self.scale.SetScaleY(&curve)
        }
    }
}

impl Renderer {
    /// Paints the DeskIsle mark's vector morph strip and click-ripple halo onto its own
    /// compositor subtree above the status bar. Cached while height, DPI, palette and
    /// shadow match; a rebuild while the menu is open preserves the open emblem state.
    pub(super) fn place_mark(
        &mut self,
        height: i32,
        dpi: u32,
        palette: Palette,
        shadow: bool,
    ) -> Result<()> {
        let key = (height, dpi, palette, shadow);
        if self.mark.as_ref().is_some_and(|mark| mark.key == key) {
            return Ok(());
        }
        let (open, hovered) = self
            .mark
            .as_ref()
            .map_or((false, false), |m| (m.open, m.hovered));
        let s = dpi as f32 / 96.;
        let (cx, cy) = (MARK_X * s, height as f32 / 2.);
        let left = (cx - MARK_BOX * 0.5 * s).floor();
        let top = (cy - MARK_BOX * 0.5 * s).floor();
        let (x, y) = (cx - left, cy - top);
        let edge = (MARK_BOX * s).ceil() as i32 + 1;
        let strip_width = edge * MARK_FRAMES as i32;

        let strip_surface = paint(&self.device, palette, strip_width, edge, dpi, |c| {
            let factory = unsafe { c.ctx.GetFactory()? };
            let stroke_style = unsafe {
                factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        dashCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.,
                        dashStyle: D2D1_DASH_STYLE_SOLID,
                        dashOffset: 0.,
                    },
                    None,
                )?
            };
            let cy_dip = y / s;
            let (sr, sa) = c.top_shadow();
            for frame in 0..MARK_FRAMES {
                let t = frame as f32 / (MARK_FRAMES - 1) as f32;
                let cx_dip = (frame as i32 * edge) as f32 / s + x / s;
                let shape = MarkShape::at(t);
                if shadow {
                    paint_shape(
                        &factory,
                        &stroke_style,
                        c,
                        cx_dip,
                        cy_dip + TOP_SHADOW_Y,
                        shape,
                        sr,
                        sa,
                    )?;
                }
                paint_shape(
                    &factory,
                    &stroke_style,
                    c,
                    cx_dip,
                    cy_dip,
                    shape,
                    c.p.accent,
                    1.,
                )?;
            }
            Ok(())
        })?;

        let pulse_surface = paint(&self.device, palette, edge, edge, dpi, |c| {
            c.ring_alpha(x / s, y / s, MARK_RADIUS, 1.15, c.p.accent, 1.)
        })?;

        let morph_goal = if open { 1. } else { 0. };
        let initial_frame = if open { MARK_FRAMES - 1 } else { 0 };
        let initial_scale = if open || !hovered {
            1.
        } else {
            MARK_HOVER_SCALE
        };

        unsafe {
            if let Some(old) = self.mark.take() {
                self.top.root.RemoveVisual(&old.visual)?;
            }
            let visual = self.device.CreateVisual()?;
            visual.SetOffsetX2(left)?;
            visual.SetOffsetY2(top)?;

            // Expanding ripple ring behind the emblem; invisible at rest.
            let pulse = self.device.CreateVisual()?;
            pulse.SetContent(&pulse_surface)?;
            let pulse_scale = self.device.CreateScaleTransform()?;
            pulse_scale.SetCenterX2(x)?;
            pulse_scale.SetCenterY2(y)?;
            pulse_scale.SetScaleX2(1.)?;
            pulse_scale.SetScaleY2(1.)?;
            pulse.SetTransform(&pulse_scale)?;
            let pulse_fade = self.device.CreateEffectGroup()?;
            pulse_fade.SetOpacity2(0.)?;
            pulse.SetEffect(&pulse_fade)?;
            visual.AddVisual(&pulse, true, None)?;

            // Separate the clip node from the scaled parent so shrinking below 1.0 never
            // pulls neighboring frames of the strip inside the viewport bounds.
            let scaled = self.device.CreateVisual()?;
            let scale = self.device.CreateScaleTransform()?;
            scale.SetCenterX2(x)?;
            scale.SetCenterY2(y)?;
            scale.SetScaleX2(initial_scale)?;
            scale.SetScaleY2(initial_scale)?;
            scaled.SetTransform(&scale)?;

            let viewport = self.device.CreateVisual()?;
            viewport.SetClip2(&D2D_RECT_F {
                left: 0.,
                top: 0.,
                right: edge as f32,
                bottom: edge as f32,
            })?;

            let strip = self.device.CreateVisual()?;
            strip.SetContent(&strip_surface)?;
            strip.SetOffsetX2(-((initial_frame as i32 * edge) as f32))?;
            viewport.AddVisual(&strip, true, None)?;
            scaled.AddVisual(&viewport, true, None)?;
            visual.AddVisual(&scaled, true, None)?;

            self.top.root.AddVisual(&visual, true, None)?;
            self.mark = Some(MarkLayer {
                visual,
                strip,
                scale,
                pulse_scale,
                pulse_fade,
                edge,
                morph: MorphTrack::new(morph_goal),
                size: CurveTrack::new(initial_scale),
                hovered,
                pressed: false,
                open,
                key,
            });
        }
        Ok(())
    }

    /// Updates the mark's subtle hover expansion as part of [`Renderer::draw_top`]
    /// before its `Commit`. Ignored while a press or open menu owns the animation.
    pub(super) fn sync_mark_hover(&mut self, hovered: bool) -> Result<()> {
        let Some(mark) = self.mark.as_mut() else {
            return Ok(());
        };
        if mark.hovered == hovered {
            return Ok(());
        }
        mark.hovered = hovered;
        if mark.open || mark.pressed {
            return Ok(());
        }
        let from = mark.size.value();
        let goal = mark.rest_scale();
        if (goal - from).abs() >= 1e-3 {
            mark.apply_scale(&self.device, vec![(0., from), (HOVER_DURATION, goal)])?;
        }
        Ok(())
    }

    /// Records whether the pointer still sits on the mark just before closing its menu,
    /// so the close settle targets the right resting scale in one curve.
    pub fn set_mark_hovered_state(&mut self, hovered: bool) {
        if let Some(mark) = self.mark.as_mut() {
            mark.hovered = hovered;
        }
    }

    /// Tactile inward compression and pre-morph while the left mouse button is held over
    /// the mark. Releasing outside the mark relaxes smoothly back to rest.
    pub fn press_mark(&mut self, pressed: bool) -> Result<()> {
        let Some(mark) = self.mark.as_mut() else {
            return Ok(());
        };
        if mark.open || mark.pressed == pressed {
            return Ok(());
        }
        mark.pressed = pressed;
        let from = mark.size.value();
        let (morph_goal, morph_dur, scale_goal, scale_dur) = if pressed {
            (
                MARK_PRESS_MORPH,
                PRESS_IN_DURATION,
                MARK_PRESS_SCALE,
                PRESS_IN_DURATION,
            )
        } else {
            (
                0.,
                PRESS_OUT_DURATION,
                mark.rest_scale(),
                PRESS_OUT_DURATION,
            )
        };
        let mut changed = mark.apply_morph(&self.device, morph_goal, morph_dur)?;
        if (scale_goal - from).abs() >= 1e-3 {
            mark.apply_scale(&self.device, vec![(0., from), (scale_dur, scale_goal)])?;
            changed = true;
        }
        if changed {
            unsafe { self.device.Commit()? };
        }
        Ok(())
    }

    /// Unfolds the DeskIsle mark into a four-pointed compass diamond with a star-diamond
    /// isle core (`❖`) while its system menu is `open`, then folds it back to a calm ring
    /// when the menu closes. Only compositor properties change; the bar is not repainted.
    pub fn focus_mark(&mut self, open: bool) -> Result<()> {
        let Some(mark) = self.mark.as_mut() else {
            return Ok(());
        };
        if mark.open == open {
            return Ok(());
        }
        mark.open = open;
        mark.pressed = false;
        let from_scale = mark.size.value();
        if open {
            mark.apply_morph(&self.device, 1., OPEN_DURATION)?;
            // A held press already compressed the mark; release straight into the bloom.
            // A rapid tap dips briefly first so it still feels tactile.
            let spring = if from_scale <= 0.92 {
                vec![(0., from_scale), (0.13, 1.08), (0.25, 1.)]
            } else {
                vec![(0., from_scale), (0.055, 0.88), (0.17, 1.08), (0.28, 1.)]
            };
            mark.apply_scale(&self.device, spring)?;
            unsafe {
                let wave = ease(&self.device, 0.94, 1.58, PULSE_DURATION)?;
                let fade = ease(&self.device, 0.38, 0., PULSE_DURATION)?;
                mark.pulse_scale.SetScaleX(&wave)?;
                mark.pulse_scale.SetScaleY(&wave)?;
                mark.pulse_fade.SetOpacity(&fade)?;
            }
        } else {
            mark.apply_morph(&self.device, 0., CLOSE_DURATION)?;
            let rest = mark.rest_scale();
            let dip = (rest * 0.95).min(from_scale);
            mark.apply_scale(
                &self.device,
                vec![(0., from_scale), (0.08, dip), (CLOSE_DURATION, rest)],
            )?;
        }
        unsafe { self.device.Commit() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2};
    use windows::Win32::Graphics::{
        Direct2D::D2D1CreateDevice, Direct3D::D3D_DRIVER_TYPE_WARP, Dxgi::IDXGIDevice,
    };
    use windows::core::Interface;

    #[test]
    fn morph_interpolates_from_exact_circle_to_compass_diamond_with_clear_moat() {
        let rest = MarkShape::at(0.);
        assert!(rest.core.is_none());
        assert!((rest.rim_stroke - MARK_STROKE).abs() < 1e-6);
        for step in 0..64 {
            let theta = step as f32 * TAU / 64.;
            let (p, _) = rest.rim.sample(theta);
            let radius = p.0.hypot(p.1);
            assert!((radius - MARK_RADIUS).abs() < 1e-5);
        }

        let pressed = MarkShape::at(MARK_PRESS_MORPH);
        assert!(pressed.core.is_some());
        assert!(pressed.rim.quad > 0.4);
        assert!(pressed.rim_stroke > MARK_STROKE);

        let open = MarkShape::at(1.);
        let (core, core_alpha) = open.core.expect("open state shows inner isle");
        assert!((core_alpha - 1.).abs() < 1e-6);
        assert!(open.rim.angle.abs() < 1e-6);
        assert!(core.angle.abs() < 1e-6);

        // Cardinal tips (N, E, S, W) extend past the resting circle; diagonal waists pull in.
        for k in 0..4 {
            let cardinal = k as f32 * FRAC_PI_2;
            let diagonal = cardinal + FRAC_PI_4;
            let tip = open.rim.radius(cardinal);
            let waist = open.rim.radius(diagonal);
            assert!(tip > 6.8 && tip + open.rim_stroke * 0.5 < MARK_BOX * 0.5 - 2.);
            assert!(waist < 4.7);

            // Keep a generous negative-space channel between the core and outer rim at all angles.
            let rim_inner_tip = tip - open.rim_stroke * 0.5;
            let rim_inner_waist = waist - open.rim_stroke * 0.5;
            assert!(rim_inner_tip - core.radius(cardinal) > 3.2);
            assert!(rim_inner_waist - core.radius(diagonal) > 2.2);
        }
    }

    #[test]
    fn step_schedule_crossings_are_strictly_increasing_in_both_directions() {
        for (from, goal, duration) in [
            (0., MARK_PRESS_MORPH, PRESS_IN_DURATION),
            (MARK_PRESS_MORPH, 1., OPEN_DURATION),
            (0., 1., OPEN_DURATION),
            (1., 0., CLOSE_DURATION),
            (0.63, 0.12, CLOSE_DURATION),
        ] {
            let (start, end, steps) = step_schedule(from, goal, duration);
            assert_ne!(start, end);
            assert_eq!(steps.len(), start.abs_diff(end));
            assert_eq!(steps.last().map(|&(_, idx)| idx), Some(end));
            let mut prev = 0.;
            for (t, _) in steps {
                assert!(t > prev && t < duration);
                prev = t;
            }
        }
        let (_, _, none) = step_schedule(0.5, 0.5, OPEN_DURATION);
        assert!(none.is_empty());

        let track = CurveTrack {
            keys: vec![(0., 0.86), (0.13, 1.08), (0.25, 1.)],
            start: Instant::now(),
        };
        assert!((track.sample_at(0.) - 0.86).abs() < 1e-5);
        assert!((track.sample_at(0.13) - 1.08).abs() < 1e-5);
        assert!((track.sample_at(0.40) - 1.).abs() < 1e-5);
    }

    #[test]
    fn rendered_frames_keep_hollow_rest_ring_and_faceted_open_isle_across_dpis() -> Result<()> {
        let dxgi: IDXGIDevice = super::super::d3d_device(D3D_DRIVER_TYPE_WARP)?.cast()?;
        let d2d = unsafe { D2D1CreateDevice(&dxgi, None)? };
        let palette = Palette::new(true, &Settings::default());

        for dpi in [96_u32, 144, 192] {
            let s = dpi as f32 / 96.;
            let edge = (MARK_BOX * s).ceil() as i32 + 1;
            let mid = edge as f32 * 0.5 / s;
            let render_frame = |t: f32| {
                super::super::canvas::paint_pixels(&d2d, palette, edge, edge, dpi, |c| {
                    let factory = unsafe { c.ctx.GetFactory()? };
                    let style = unsafe {
                        factory.CreateStrokeStyle(
                            &D2D1_STROKE_STYLE_PROPERTIES {
                                startCap: D2D1_CAP_STYLE_ROUND,
                                endCap: D2D1_CAP_STYLE_ROUND,
                                lineJoin: D2D1_LINE_JOIN_ROUND,
                                miterLimit: 10.,
                                ..Default::default()
                            },
                            None,
                        )?
                    };
                    paint_shape(
                        &factory,
                        &style,
                        c,
                        mid,
                        mid,
                        MarkShape::at(t),
                        c.p.accent,
                        1.,
                    )
                })
            };
            let rest = render_frame(0.)?;
            let open = render_frame(1.)?;
            let alpha = |buf: &[u8], dx_dip: f32, dy_dip: f32| {
                let px = ((mid + dx_dip) * s).floor().clamp(0., (edge - 1) as f32) as i32;
                let py = ((mid + dy_dip) * s).floor().clamp(0., (edge - 1) as f32) as i32;
                buf[((py * edge + px) * 4 + 3) as usize]
            };

            // Resting frame: empty center, solid coverage along the 6 DIP circle.
            assert_eq!(alpha(&rest, 0., 0.), 0);
            assert!(alpha(&rest, 0., -MARK_RADIUS) > 180);
            assert!(
                alpha(
                    &rest,
                    MARK_RADIUS * FRAC_1_SQRT_2,
                    -MARK_RADIUS * FRAC_1_SQRT_2
                ) > 180
            );

            // Open frame: filled star-diamond core, clear negative-space moat, cardinal diamond
            // tip reaching further than the retracted diagonal waist.
            assert_eq!(alpha(&open, 0., 0.), 255);
            assert_eq!(alpha(&open, 0., -4.28), 0);
            assert!(alpha(&open, 0., -6.92) > 180);
            assert_eq!(alpha(&open, 4.6, -4.6), 0);
            assert!(alpha(&open, 3.22, -3.22) > 180);
        }
        Ok(())
    }
}
