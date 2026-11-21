//! DeskIsle's own icon set, shared by the status bar, the menus and flyouts, and the
//! settings panel. Every icon is outlined on a 24-unit grid with round caps and joins and
//! a stroke weight that follows the drawn size, so the set reads as one family at every
//! scale. Outlines are plain values; the renderer turns them into geometry.

/// One icon of the set. Icons that show a level carry it, so a change redraws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    // The status bar and its flyouts.
    Wifi,
    Cellular,
    Ethernet,
    Bluetooth,
    Display,
    /// The charge in percent, or `None` when the battery does not report it.
    Battery {
        percent: Option<u8>,
        charging: bool,
    },
    /// Sound waves lit from 0 (silent) to 3 (loud).
    Volume(u8),
    Muted,
    /// Two switches, the control center's mark.
    Controls,
    Keyboard,
    /// The tray overflow button while its menu is closed: more icons below.
    ChevronDown,
    /// The tray overflow button while its menu is open.
    ChevronUp,
    /// The calendar's previous and next month.
    ChevronLeft,
    ChevronRight,
    Sun,
    Moon,
    Contrast,
    Hdr,
    Speaker,
    Bolt,
    Cast,
    Music,
    Previous,
    Play,
    Pause,
    Next,
    Settings,
    // Menu commands.
    Open,
    Folder,
    Plus,
    Minus,
    Pin,
    Unpin,
    Close,
    Check,
    Apps,
    Search,
    Activity,
    Desktop,
    Power,
    Info,
    Sliders,
    Store,
    Restart,
    Lock,
    /// Running with administrator rights, as the Windows UAC shield.
    Shield,
    SignOut,
    Calendar,
    Bell,
    Window,
    // Settings panel pages and monitor rows.
    Palette,
    Dock,
    Computer,
    Clock,
    Chip,
    Disk,
    Thermometer,
    GraphicsCard,
    Memory,
    MeterBars(u8),
    MeterRing(u8),
}

/// Width and height of the grid icons are outlined on.
pub const GRID: f32 = 24.;

/// A battery at or below this charge, and not charging, shows its level in the alert color.
pub const LOW_BATTERY: u8 = 15;

/// Stroke width in DIPs for an icon drawn `size` DIPs square: about 1/12 of the size,
/// never so thin that it blurs at 100% scale nor so heavy that large icons look bold.
pub fn weight(size: f32) -> f32 {
    (size * 0.085).clamp(1.25, 2.25)
}

pub type Point = (f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Stroke,
    Fill,
}

/// Which color a shape takes: the caller's ink, or the palette's alert color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ink,
    Alert,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Line(Point),
    Quad {
        control: Point,
        to: Point,
    },
    /// An elliptical arc as in SVG, with equal radii.
    Arc {
        to: Point,
        radius: f32,
        clockwise: bool,
        large: bool,
    },
}

/// One figure of an icon in grid units.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub start: Point,
    pub steps: Vec<Step>,
    pub closed: bool,
    pub paint: Paint,
    pub tone: Tone,
    /// Multiplies the caller's opacity, for unlit parts such as quiet sound waves.
    pub alpha: f32,
}

/// The figures of `symbol`, in drawing order.
pub fn sketch(symbol: Symbol) -> Vec<Shape> {
    use Paint::{Fill, Stroke};
    let mut s = Sketch(Vec::new());
    match symbol {
        Symbol::Keyboard => {
            s.rect(1.75, 5., 22.25, 19., 2.5, Stroke);
            for y in [9., 12.] {
                for x in [6., 10., 14., 18.] {
                    s.circle(x, y, 0.9, Fill);
                }
            }
            s.line(&[(7.5, 15.5), (16.5, 15.5)]);
        }
        Symbol::Search => {
            s.circle(10., 10., 6.5, Stroke);
            s.line(&[(15., 15.), (20.5, 20.5)]);
        }
        Symbol::Wifi => {
            for radius in [5., 9.5, 14.] {
                s.arc(12., 18.5, radius, 225., 315.);
            }
            s.circle(12., 18.5, 1.7, Fill);
        }
        Symbol::Cellular => {
            for (left, top) in [(3., 15.), (8., 11.5), (13., 8.), (18., 4.5)] {
                s.rect(left, top, left + 3., 19.5, 1., Fill);
            }
        }
        Symbol::Ethernet => {
            s.rect(9., 3., 15., 8.5, 1.5, Stroke);
            s.rect(3., 15.5, 9., 21., 1.5, Stroke);
            s.rect(15., 15.5, 21., 21., 1.5, Stroke);
            s.line(&[(12., 8.5), (12., 12.)]);
            s.polyline(
                &[(6., 15.5), (6., 12.), (18., 12.), (18., 15.5)],
                1.5,
                false,
                Stroke,
            );
        }
        Symbol::Bluetooth => {
            s.line(&[
                (6.5, 7.5),
                (17., 16.5),
                (12., 21.),
                (12., 3.),
                (17., 7.5),
                (6.5, 16.5),
            ]);
        }
        Symbol::Display => {
            s.rect(2.5, 3.5, 21.5, 16., 2.5, Stroke);
            s.line(&[(12., 16.), (12., 20.5)]);
            s.line(&[(8., 20.5), (16., 20.5)]);
        }
        Symbol::Battery { percent, charging } => {
            s.rect(1.5, 6.5, 19.5, 17.5, 3.5, Stroke);
            s.rect(21., 10., 22.75, 14., 0.9, Fill);
            if let Some(percent) = percent.filter(|&p| p > 0) {
                let width = (13. * f32::from(percent.min(100)) / 100.).max(2.);
                let level = s.rect(4., 9., 4. + width, 15., 1.5, Fill);
                if charging {
                    // Faded so the bolt on top stays legible at any charge.
                    level.alpha = 0.35;
                } else if percent <= LOW_BATTERY {
                    level.tone = Tone::Alert;
                }
            }
            if charging {
                s.polyline(
                    &[
                        (11.5, 7.5),
                        (7.5, 12.8),
                        (10.3, 12.8),
                        (9.5, 16.5),
                        (13.5, 11.2),
                        (10.7, 11.2),
                    ],
                    0.6,
                    true,
                    Fill,
                );
            }
        }
        Symbol::Volume(_) | Symbol::Muted => {
            s.polyline(
                &[
                    (2., 9.),
                    (5.5, 9.),
                    (10., 4.5),
                    (10., 19.5),
                    (5.5, 15.),
                    (2., 15.),
                ],
                1.,
                true,
                Stroke,
            );
            if let Symbol::Volume(level) = symbol {
                for (wave, (radius, angle)) in
                    [(4., 40.), (7.5, 45.), (11., 45.)].into_iter().enumerate()
                {
                    let lit = wave < usize::from(level.min(3));
                    s.arc(10., 12., radius, -angle, angle).alpha = if lit { 1. } else { 0.3 };
                }
            } else {
                s.line(&[(15., 9.), (21., 15.)]);
                s.line(&[(21., 9.), (15., 15.)]);
            }
        }
        Symbol::Controls => {
            // The upper switch is on: a tinted track with its knob at the end.
            s.rect(2.5, 3.5, 21.5, 11., 3.75, Fill).alpha = 0.18;
            s.rect(2.5, 3.5, 21.5, 11., 3.75, Stroke);
            s.circle(17.75, 7.25, 1.75, Fill);
            s.rect(2.5, 13., 21.5, 20.5, 3.75, Stroke);
            s.circle(6.25, 16.75, 1.75, Fill).alpha = 0.45;
        }
        // Mirror images around the grid's middle, so the button keeps its place as it flips.
        Symbol::ChevronDown => {
            s.line(&[(6.5, 9.25), (12., 14.75), (17.5, 9.25)]);
        }
        Symbol::ChevronUp => {
            s.line(&[(6.5, 14.75), (12., 9.25), (17.5, 14.75)]);
        }
        Symbol::ChevronLeft => {
            s.line(&[(14.75, 6.5), (9.25, 12.), (14.75, 17.5)]);
        }
        Symbol::ChevronRight => {
            s.line(&[(9.25, 6.5), (14.75, 12.), (9.25, 17.5)]);
        }
        Symbol::Sun => {
            s.circle(12., 12., 4., Stroke);
            for ray in 0..8 {
                let angle = ray as f32 * 45.;
                s.line(&[polar(12., 12., 7.5, angle), polar(12., 12., 9.5, angle)]);
            }
        }
        Symbol::Moon => {
            // A disc of radius 8.5 around the center with a bite out of its upper right.
            s.push(
                (12.5, 3.5),
                vec![
                    arc_to((20.5, 11.5), 6.5, false, false),
                    arc_to((12.5, 3.5), 8.5, true, true),
                ],
                true,
                Stroke,
            );
        }
        Symbol::Contrast => {
            s.circle(12., 12., 9., Stroke);
            s.push(
                (12., 3.),
                vec![arc_to((12., 21.), 9., true, false)],
                true,
                Fill,
            );
        }
        Symbol::Hdr => {
            s.rect(2.5, 5., 21.5, 19., 3., Stroke);
            s.polyline(&[(18., 8.5), (18., 15.5), (6., 15.5)], 1., true, Fill);
        }
        Symbol::Speaker => {
            s.rect(5.5, 2.5, 18.5, 21.5, 3., Stroke);
            s.circle(12., 14.5, 3.5, Stroke);
            s.circle(12., 7., 1.25, Fill);
        }
        Symbol::Bolt => {
            s.polyline(
                &[
                    (13.5, 2.5),
                    (4.5, 13.5),
                    (11.5, 13.5),
                    (10.5, 21.5),
                    (19.5, 10.5),
                    (12.5, 10.5),
                ],
                1.,
                true,
                Stroke,
            );
        }
        Symbol::Cast => {
            s.polyline(
                &[(2.5, 8.5), (2.5, 4.), (21.5, 4.), (21.5, 20.), (14.5, 20.)],
                2.5,
                false,
                Stroke,
            );
            s.arc(3., 20.5, 4.5, -90., 0.);
            s.arc(3., 20.5, 8.5, -90., 0.);
            s.circle(3.25, 20.25, 1.4, Fill);
        }
        Symbol::Music => {
            s.line(&[(9., 18.), (9., 5.5), (20., 3.5), (20., 16.)]);
            s.circle(6.25, 18., 2.75, Fill);
            s.circle(17.25, 16., 2.75, Fill);
        }
        Symbol::Previous => {
            s.polyline(&[(19., 5.), (19., 19.), (8.5, 12.)], 1.5, true, Fill);
            s.rect(4.5, 5., 7.5, 19., 1.5, Fill);
        }
        Symbol::Play => {
            s.polyline(&[(7., 4.), (20., 12.), (7., 20.)], 2., true, Fill);
        }
        Symbol::Pause => {
            s.rect(5.5, 4., 10., 20., 1.5, Fill);
            s.rect(14., 4., 18.5, 20., 1.5, Fill);
        }
        Symbol::Next => {
            s.polyline(&[(5., 5.), (15.5, 12.), (5., 19.)], 1.5, true, Fill);
            s.rect(16.5, 5., 19.5, 19., 1.5, Fill);
        }
        Symbol::Settings => {
            let mut teeth = Vec::with_capacity(24);
            for tooth in 0..6 {
                let angle = tooth as f32 * 60. - 90.;
                teeth.extend([
                    polar(12., 12., 7.25, angle - 22.),
                    polar(12., 12., 9.75, angle - 11.),
                    polar(12., 12., 9.75, angle + 11.),
                    polar(12., 12., 7.25, angle + 22.),
                ]);
            }
            s.polyline(&teeth, 1., true, Stroke);
            s.circle(12., 12., 3., Stroke);
        }
        Symbol::Open => {
            s.polyline(
                &[
                    (10.5, 4.5),
                    (4.5, 4.5),
                    (4.5, 19.5),
                    (19.5, 19.5),
                    (19.5, 13.5),
                ],
                2.5,
                false,
                Stroke,
            );
            s.line(&[(14., 4.5), (19.5, 4.5), (19.5, 10.)]);
            s.line(&[(19.5, 4.5), (11., 13.)]);
        }
        Symbol::Folder => {
            s.polyline(
                &[
                    (2.5, 5.),
                    (9., 5.),
                    (11., 7.5),
                    (21.5, 7.5),
                    (21.5, 19.),
                    (2.5, 19.),
                ],
                2.,
                true,
                Stroke,
            );
        }
        Symbol::Plus => {
            s.line(&[(12., 5.), (12., 19.)]);
            s.line(&[(5., 12.), (19., 12.)]);
        }
        Symbol::Minus => {
            s.line(&[(5., 12.), (19., 12.)]);
        }
        Symbol::Pin | Symbol::Unpin => {
            s.polyline(
                &[
                    (9.5, 3.5),
                    (9.5, 10.),
                    (6., 14.),
                    (6., 16.5),
                    (18., 16.5),
                    (18., 14.),
                    (14.5, 10.),
                    (14.5, 3.5),
                ],
                1.5,
                false,
                Stroke,
            );
            s.line(&[(8., 3.5), (16., 3.5)]);
            s.line(&[(12., 16.5), (12., 21.5)]);
            if symbol == Symbol::Unpin {
                s.line(&[(3.5, 3.5), (20.5, 20.5)]);
            }
        }
        Symbol::Close => {
            s.line(&[(6., 6.), (18., 18.)]);
            s.line(&[(18., 6.), (6., 18.)]);
        }
        Symbol::Check => {
            s.line(&[(4.5, 12.5), (9.5, 17.5), (19.5, 6.5)]);
        }
        Symbol::Apps => {
            for (left, top) in [(3.5, 3.5), (13.5, 3.5), (3.5, 13.5), (13.5, 13.5)] {
                s.rect(left, top, left + 7., top + 7., 2., Stroke);
            }
        }
        Symbol::Activity => {
            s.line(&[
                (2.5, 12.),
                (6.5, 12.),
                (9.5, 4.5),
                (14.5, 19.5),
                (17.5, 12.),
                (21.5, 12.),
            ]);
        }
        Symbol::Desktop => {
            s.rect(2.5, 3.5, 21.5, 16., 2.5, Stroke);
            s.line(&[(12., 6.5), (12., 12.5)]);
            s.line(&[(9., 9.8), (12., 12.8), (15., 9.8)]);
            s.line(&[(12., 16.), (12., 20.5)]);
            s.line(&[(8., 20.5), (16., 20.5)]);
        }
        Symbol::Power => {
            s.arc(12., 12.5, 8.5, -50., 230.);
            s.line(&[(12., 2.5), (12., 11.)]);
        }
        Symbol::Info => {
            s.circle(12., 12., 9.5, Stroke);
            s.line(&[(12., 11.), (12., 16.5)]);
            s.circle(12., 7.75, 1.2, Fill);
        }
        Symbol::Sliders => {
            for (y, knob) in [(6., 15.5), (12., 8.5), (18., 13.)] {
                s.line(&[(3., y), (knob - 2.5, y)]);
                s.circle(knob, y, 2.5, Stroke);
                s.line(&[(knob + 2.5, y), (21., y)]);
            }
        }
        Symbol::Store => {
            s.polyline(
                &[(4., 8.), (20., 8.), (19., 21.), (5., 21.)],
                2.,
                true,
                Stroke,
            );
            s.push(
                (8.5, 11.),
                vec![
                    Step::Line((8.5, 7.)),
                    arc_to((15.5, 7.), 3.5, true, false),
                    Step::Line((15.5, 11.)),
                ],
                false,
                Stroke,
            );
        }
        Symbol::Restart => {
            s.arc(12., 13., 8., -30., 270.);
            s.head((12., 5.), 0., 4.);
        }
        Symbol::Lock => {
            s.rect(4.5, 10.5, 19.5, 21., 2.5, Stroke);
            s.push(
                (8., 10.5),
                vec![
                    Step::Line((8., 7.5)),
                    arc_to((16., 7.5), 4., true, false),
                    Step::Line((16., 10.5)),
                ],
                false,
                Stroke,
            );
            s.line(&[(12., 14.5), (12., 17.)]);
        }
        Symbol::Shield => {
            s.push(
                (12., 2.75),
                vec![
                    Step::Line((19.5, 5.5)),
                    Step::Line((19.5, 11.)),
                    Step::Quad {
                        control: (19.5, 18.),
                        to: (12., 21.25),
                    },
                    Step::Quad {
                        control: (4.5, 18.),
                        to: (4.5, 11.),
                    },
                    Step::Line((4.5, 5.5)),
                ],
                true,
                Stroke,
            );
            s.line(&[(8.75, 12.), (11.1, 14.35), (15.5, 9.75)]);
        }
        Symbol::SignOut => {
            s.polyline(
                &[(10., 3.5), (4.5, 3.5), (4.5, 20.5), (10., 20.5)],
                2.,
                false,
                Stroke,
            );
            s.line(&[(9.5, 12.), (20.5, 12.)]);
            s.line(&[(16.5, 8.), (20.5, 12.), (16.5, 16.)]);
        }
        Symbol::Calendar => {
            s.rect(3., 5., 21., 21., 2.5, Stroke);
            s.line(&[(3., 10.), (21., 10.)]);
            s.line(&[(8., 3.), (8., 7.)]);
            s.line(&[(16., 3.), (16., 7.)]);
            for (x, y) in [(8., 14.), (12., 14.), (16., 14.), (8., 17.5), (12., 17.5)] {
                s.circle(x, y, 1.15, Fill);
            }
        }
        Symbol::Bell => {
            s.push(
                (4.5, 17.5),
                vec![
                    Step::Quad {
                        control: (6.5, 16.),
                        to: (6.5, 13.),
                    },
                    Step::Line((6.5, 10.)),
                    arc_to((17.5, 10.), 5.5, true, false),
                    Step::Line((17.5, 13.)),
                    Step::Quad {
                        control: (17.5, 16.),
                        to: (19.5, 17.5),
                    },
                ],
                true,
                Stroke,
            );
            s.push(
                (10., 20.5),
                vec![Step::Quad {
                    control: (12., 22.3),
                    to: (14., 20.5),
                }],
                false,
                Stroke,
            );
            s.line(&[(12., 2.8), (12., 4.5)]);
        }
        Symbol::Window => {
            s.rect(3., 4., 21., 20., 2.5, Stroke);
            s.line(&[(3., 8.5), (21., 8.5)]);
        }
        Symbol::Palette => {
            s.circle(12., 12., 9., Stroke);
            for (x, y) in [(7.5, 10.5), (10.5, 6.8), (15., 7.3), (17., 11.5)] {
                s.circle(x, y, 1.5, Fill);
            }
            s.circle(14., 16.5, 2., Stroke);
        }
        Symbol::Dock => {
            s.rect(2.5, 3.5, 21.5, 20.5, 2.5, Stroke);
            s.rect(6., 14.5, 18., 17.5, 1.5, Fill);
        }
        Symbol::Computer => {
            s.rect(4.5, 5., 19.5, 15.5, 2., Stroke);
            s.line(&[(2.5, 19.), (21.5, 19.)]);
        }
        Symbol::Clock => {
            s.circle(12., 12., 9.5, Stroke);
            s.line(&[(12., 6.5), (12., 12.), (15.5, 14.)]);
        }
        Symbol::Chip => {
            s.rect(6., 6., 18., 18., 2.5, Stroke);
            s.rect(9.5, 9.5, 14.5, 14.5, 1., Stroke);
            for p in [9.5, 14.5] {
                s.line(&[(p, 2.5), (p, 6.)]);
                s.line(&[(p, 18.), (p, 21.5)]);
                s.line(&[(2.5, p), (6., p)]);
                s.line(&[(18., p), (21.5, p)]);
            }
        }
        Symbol::Disk => {
            s.rect(2.5, 7., 21.5, 17., 3., Stroke);
            s.line(&[(6., 12.), (11., 12.)]);
            s.circle(17.5, 12., 1.3, Fill);
        }
        Symbol::Thermometer => {
            s.push(
                (10., 14.5),
                vec![
                    Step::Line((10., 5.)),
                    arc_to((14., 5.), 2., true, false),
                    Step::Line((14., 14.5)),
                    arc_to((10., 14.5), 4., true, true),
                ],
                true,
                Stroke,
            );
            s.line(&[(12., 18.), (12., 10.)]);
            s.circle(12., 18., 1.8, Fill);
            s.line(&[(17., 6.), (19., 6.)]);
            s.line(&[(17., 9.5), (19., 9.5)]);
        }
        Symbol::Memory => {
            s.rect(2.5, 6.5, 21.5, 16.5, 2., Stroke);
            for left in [5.5, 10.25, 15.] {
                s.rect(left, 9.5, left + 3.5, 13.5, 1., Fill);
            }
            for x in [6., 10., 14., 18.] {
                s.line(&[(x, 16.5), (x, 19.5)]);
            }
        }
        Symbol::GraphicsCard => {
            // The rear bracket, cooling fan and PCIe contacts stay distinct at 16 DIPs.
            s.line(&[(2., 4.), (4.5, 4.), (4.5, 20.)]);
            s.rect(4.5, 6., 21.5, 16., 2., Stroke);
            s.circle(15.5, 11., 2.7, Stroke);
            s.line(&[(7.5, 10.), (9.5, 10.)]);
            s.rect(7., 16., 13., 18.5, 0.7, Fill);
        }
        Symbol::MeterBars(percent) => {
            for index in 0..5 {
                let left = 1.5 + index as f32 * 4.5;
                let top = 14. - index as f32 * 2.5;
                s.rect(left, top, left + 3., 19., 1.25, Fill).alpha = 0.23;
                let used = (f32::from(percent) / 20. - index as f32).clamp(0., 1.);
                if used > 0. {
                    s.rect(left, 19. - (19. - top) * used, left + 3., 19., 1.25, Fill);
                }
            }
        }
        Symbol::MeterRing(percent) => {
            s.circle(12., 12., 8.5, Stroke).alpha = 0.23;
            match percent {
                0 => {}
                100.. => {
                    s.circle(12., 12., 8.5, Stroke);
                }
                level => {
                    s.arc(12., 12., 8.5, -90., -90. + f32::from(level) * 3.6);
                }
            }
        }
    }
    s.0
}

/// A trace through consecutive samples; a failed read leaves a gap in the line.
/// Use a point for an isolated reading so the graph is legible from its first sample.
pub fn history_trace(history: &[Option<u8>]) -> Vec<Shape> {
    let mut s = Sketch(Vec::new());
    let mut run = Vec::new();
    let dx = 22. / history.len().saturating_sub(1).max(1) as f32;
    for (index, value) in history.iter().chain([&None]).enumerate() {
        if let Some(value) = value {
            let x = 23. - history.len().saturating_sub(index + 1) as f32 * dx;
            run.push((x, 19. - f32::from((*value).min(100)) * 0.14));
        } else {
            match run.as_slice() {
                [] => {}
                &[(x, y)] => {
                    s.circle(x, y, 0.8, Paint::Fill);
                }
                points => {
                    s.line(points);
                }
            }
            run.clear();
        }
    }
    s.0
}

fn arc_to(to: Point, radius: f32, clockwise: bool, large: bool) -> Step {
    Step::Arc {
        to,
        radius,
        clockwise,
        large,
    }
}

/// The point `radius` from the center at `degrees` clockwise from the positive x axis.
fn polar(x: f32, y: f32, radius: f32, degrees: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (x + radius * cos, y + radius * sin)
}

fn distance(a: Point, b: Point) -> f32 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// The point `length` from `from` toward `to`.
fn toward(from: Point, to: Point, length: f32) -> Point {
    let d = distance(from, to);
    if d == 0. {
        return from;
    }
    (
        from.0 + (to.0 - from.0) / d * length,
        from.1 + (to.1 - from.1) / d * length,
    )
}

struct Sketch(Vec<Shape>);

impl Sketch {
    fn push(&mut self, start: Point, steps: Vec<Step>, closed: bool, paint: Paint) -> &mut Shape {
        self.0.push(Shape {
            start,
            steps,
            closed,
            paint,
            tone: Tone::Ink,
            alpha: 1.,
        });
        self.0.last_mut().expect("a shape was just pushed")
    }

    /// Straight segments through `points`, open and stroked.
    fn line(&mut self, points: &[Point]) -> &mut Shape {
        let steps = points[1..].iter().map(|&p| Step::Line(p)).collect();
        self.push(points[0], steps, false, Paint::Stroke)
    }

    /// Segments through `points` whose corners are rounded by up to `radius`, never past
    /// the middle of a segment.
    fn polyline(
        &mut self,
        points: &[Point],
        radius: f32,
        closed: bool,
        paint: Paint,
    ) -> &mut Shape {
        let n = points.len();
        let corner = |i: usize| {
            let at = points[i];
            let before = points[(i + n - 1) % n];
            let after = points[(i + 1) % n];
            let r = radius
                .min(distance(at, before) / 2.)
                .min(distance(at, after) / 2.);
            (toward(at, before, r), toward(at, after, r))
        };
        let (start, corners): (Point, Vec<usize>) = if closed {
            (corner(0).1, (1..n).chain([0]).collect())
        } else {
            (points[0], (1..n - 1).collect())
        };
        let mut steps = Vec::with_capacity(2 * n);
        for i in corners {
            let (enter, leave) = corner(i);
            steps.push(Step::Line(enter));
            steps.push(Step::Quad {
                control: points[i],
                to: leave,
            });
        }
        if !closed {
            steps.push(Step::Line(points[n - 1]));
        }
        self.push(start, steps, closed, paint)
    }

    fn rect(&mut self, l: f32, t: f32, r: f32, b: f32, radius: f32, paint: Paint) -> &mut Shape {
        self.polyline(&[(l, t), (r, t), (r, b), (l, b)], radius, true, paint)
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, paint: Paint) -> &mut Shape {
        self.push(
            (x + radius, y),
            vec![
                arc_to((x - radius, y), radius, true, false),
                arc_to((x + radius, y), radius, true, false),
            ],
            true,
            paint,
        )
    }

    /// A stroked arc from `from` to `to` degrees, clockwise when `to` is larger.
    fn arc(&mut self, x: f32, y: f32, radius: f32, from: f32, to: f32) -> &mut Shape {
        let sweep = to - from;
        self.push(
            polar(x, y, radius, from),
            vec![arc_to(
                polar(x, y, radius, to),
                radius,
                sweep > 0.,
                sweep.abs() > 180.,
            )],
            false,
            Paint::Stroke,
        )
    }

    /// An open arrowhead at `tip`, pointing `direction` degrees clockwise from the x axis.
    fn head(&mut self, tip: Point, direction: f32, length: f32) -> &mut Shape {
        self.line(&[
            polar(tip.0, tip.1, length, direction + 140.),
            tip,
            polar(tip.0, tip.1, length, direction + 220.),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Symbol] = &[
        Symbol::Wifi,
        Symbol::Cellular,
        Symbol::Ethernet,
        Symbol::Bluetooth,
        Symbol::Display,
        Symbol::Battery {
            percent: None,
            charging: false,
        },
        Symbol::Battery {
            percent: Some(0),
            charging: true,
        },
        Symbol::Battery {
            percent: Some(100),
            charging: false,
        },
        Symbol::Volume(0),
        Symbol::Volume(3),
        Symbol::Muted,
        Symbol::Controls,
        Symbol::ChevronDown,
        Symbol::ChevronUp,
        Symbol::ChevronLeft,
        Symbol::ChevronRight,
        Symbol::Sun,
        Symbol::Moon,
        Symbol::Contrast,
        Symbol::Hdr,
        Symbol::Speaker,
        Symbol::Bolt,
        Symbol::Cast,
        Symbol::Music,
        Symbol::Previous,
        Symbol::Play,
        Symbol::Pause,
        Symbol::Next,
        Symbol::Settings,
        Symbol::Open,
        Symbol::Folder,
        Symbol::Plus,
        Symbol::Minus,
        Symbol::Pin,
        Symbol::Unpin,
        Symbol::Close,
        Symbol::Check,
        Symbol::Apps,
        Symbol::Search,
        Symbol::Keyboard,
        Symbol::Activity,
        Symbol::Desktop,
        Symbol::Power,
        Symbol::Info,
        Symbol::Sliders,
        Symbol::Store,
        Symbol::Restart,
        Symbol::Lock,
        Symbol::Shield,
        Symbol::SignOut,
        Symbol::Calendar,
        Symbol::Bell,
        Symbol::Window,
        Symbol::Palette,
        Symbol::Dock,
        Symbol::Computer,
        Symbol::Clock,
        Symbol::Chip,
        Symbol::Disk,
        Symbol::Thermometer,
        Symbol::Memory,
        Symbol::GraphicsCard,
        Symbol::MeterBars(0),
        Symbol::MeterBars(63),
        Symbol::MeterRing(0),
        Symbol::MeterRing(63),
        Symbol::MeterRing(100),
    ];

    fn points(shape: &Shape) -> Vec<Point> {
        let mut found = vec![shape.start];
        for step in &shape.steps {
            match *step {
                Step::Line(p) => found.push(p),
                Step::Quad { control, to } => found.extend([control, to]),
                Step::Arc { to, .. } => found.push(to),
            }
        }
        found
    }

    #[test]
    fn every_icon_stays_inside_the_grid_with_room_for_its_stroke() {
        for &symbol in ALL {
            let shapes = sketch(symbol);
            assert!(!shapes.is_empty(), "{symbol:?}");
            for shape in &shapes {
                for (x, y) in points(shape) {
                    let inside = |v: f32| (1.0..=GRID - 1.).contains(&v);
                    assert!(inside(x) && inside(y), "{symbol:?} reaches ({x}, {y})");
                }
                for step in &shape.steps {
                    if let Step::Arc { radius, .. } = step {
                        assert!(*radius > 0., "{symbol:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn louder_volume_lights_more_waves() {
        let lit = |level| {
            sketch(Symbol::Volume(level))
                .iter()
                .filter(|s| s.alpha == 1.)
                .count()
        };
        assert_eq!(lit(0), 1);
        assert!(lit(0) < lit(1) && lit(1) < lit(2) && lit(2) < lit(3));
        assert_eq!(lit(3), lit(9));
    }

    #[test]
    fn trace_breaks_at_missing_samples_and_places_newest_on_the_right() {
        let shapes = history_trace(&[Some(0), Some(100), None, Some(50)]);
        assert_eq!(shapes.len(), 2);
        let trace = points(&shapes[0]);
        assert!(trace[0].0 < trace[1].0 && trace[0].1 > trace[1].1);
        assert_eq!(shapes[1].paint, Paint::Fill);
        assert!(shapes[1].start.0 > trace[1].0);
        assert!(history_trace(&[None, None]).is_empty());
    }

    #[test]
    fn battery_level_follows_the_charge_and_warns_when_low() {
        let level_width = |percent| {
            let shapes = sketch(Symbol::Battery {
                percent: Some(percent),
                charging: false,
            });
            let level = &shapes[2];
            let xs: Vec<f32> = points(level).iter().map(|p| p.0).collect();
            xs.iter().copied().fold(f32::MIN, f32::max)
                - xs.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!(level_width(30) < level_width(80));
        let low = |charging| {
            sketch(Symbol::Battery {
                percent: Some(LOW_BATTERY),
                charging,
            })
            .iter()
            .any(|s| s.tone == Tone::Alert)
        };
        assert!(low(false));
        assert!(!low(true));
        // Nothing to fill without a reported charge.
        let unknown = sketch(Symbol::Battery {
            percent: None,
            charging: false,
        });
        assert_eq!(unknown.len(), 2);
    }

    #[test]
    fn overflow_chevrons_mirror_each_other() {
        let flipped: Vec<Point> = points(&sketch(Symbol::ChevronDown)[0])
            .into_iter()
            .map(|(x, y)| (x, GRID - y))
            .collect();
        assert_eq!(flipped, points(&sketch(Symbol::ChevronUp)[0]));
    }

    #[test]
    fn stroke_weight_grows_with_size_within_bounds() {
        assert_eq!(weight(8.), 1.25);
        assert!(weight(16.) > weight(14.));
        assert_eq!(weight(64.), 2.25);
    }
}
