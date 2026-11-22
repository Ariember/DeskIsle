use windows::Win32::Graphics::{
    DirectComposition::IDCompositionVisual2,
    DirectWrite::{DWRITE_TEXT_ALIGNMENT_TRAILING, IDWriteFactory},
};
use windows::core::Result;

use super::canvas::{Canvas, Palette, Text, paint};
use super::{Dimensions, Renderer};
use crate::metrics::{Indicator, Metric, Style};
use crate::ui::icon::{self, Symbol};
use crate::ui::layout::{self, StatusLayout};

/// A transparent strip above the bar's glass. Sampling only repaints this strip;
/// its tray neighbors keep their existing images.
pub(super) struct MetricLayer {
    visual: IDCompositionVisual2,
    size: Dimensions,
    left: i32,
    palette: Palette,
    readings: Vec<Indicator>,
}

impl Renderer {
    pub fn draw_metrics(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        metrics: &[Indicator],
    ) -> Result<()> {
        let caption = self
            .top_state
            .as_ref()
            .and_then(|state| state.immersive_color);
        let palette = self.palette.for_top_bar(caption);
        if self.update_metrics(layout, dpi, metrics, palette)? {
            unsafe { self.device.Commit()? };
        }
        Ok(())
    }

    pub(super) fn update_metrics(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        readings: &[Indicator],
        palette: Palette,
    ) -> Result<bool> {
        let slots = &layout.metrics;
        let (Some(first), Some(last)) = (slots.first(), slots.last()) else {
            if let Some(layer) = self.metric_layer.take() {
                unsafe { self.top.root.RemoveVisual(&layer.visual)? };
                return Ok(true);
            }
            return Ok(false);
        };
        let readings = &readings[..slots.len()];
        let size = (last.x + last.width - first.x, layout.height, dpi);
        if let Some(layer) = &mut self.metric_layer
            && (layer.size, layer.palette) == (size, palette)
            && layer.readings == readings
        {
            if first.x == layer.left {
                return Ok(false);
            }
            unsafe { layer.visual.SetOffsetX2(first.x as f32)? };
            layer.left = first.x;
            return Ok(true);
        }
        let surface = paint(&self.device, palette, size.0, size.1, dpi, |c| {
            for (r, indicator) in slots.iter().zip(readings) {
                let l = (r.x - first.x) as f32 / c.scale;
                let right = (r.x + r.width - first.x) as f32 / c.scale;
                paint_indicator(c, &self.write, indicator, l, right, size.1 as f32 / c.scale)?;
            }
            Ok(())
        })?;
        unsafe {
            let visual = match &self.metric_layer {
                Some(layer) => layer.visual.clone(),
                None => self.device.CreateVisual()?,
            };
            visual.SetContent(&surface)?;
            visual.SetOffsetX2(first.x as f32)?;
            if self.metric_layer.is_none() {
                self.top.root.AddVisual(&visual, true, None)?;
            }
            self.metric_layer = Some(MetricLayer {
                visual,
                size,
                left: first.x,
                palette,
                readings: readings.to_owned(),
            });
        }
        Ok(true)
    }
}

fn paint_indicator(
    c: &Canvas,
    write: &IDWriteFactory,
    indicator: &Indicator,
    left: f32,
    right: f32,
    height: f32,
) -> Result<()> {
    let symbol = match indicator.metric {
        Metric::Cpu => Symbol::Chip,
        Metric::Memory => Symbol::Memory,
        Metric::Disk => Symbol::Disk,
        Metric::CpuTemperature => Symbol::Thermometer,
        Metric::GpuTemperature => Symbol::GraphicsCard,
    };
    let ink = if indicator.highlight {
        c.p.danger
    } else {
        c.p.ink
    };
    let cy = height / 2.;
    // The slot pads its content by half a gap on each side: the symbol starts the
    // content and the readout ends it.
    let (icon, pad) = (layout::BAR_ICON as f32, layout::BAR_GAP as f32 / 2.);
    let start = left + pad + icon;
    let end = right - pad;
    c.symbol(symbol, start - icon / 2., cy, icon, ink, 1.)?;
    // Readings align to the end; the spacing before the readout absorbs small
    // differences between the reserved and the drawn text.
    let text = |value: &str, color| {
        c.draw_text(
            write,
            value,
            c.rect(start + 1., 0., end, height),
            Text {
                size: layout::METRIC_TEXT,
                align: DWRITE_TEXT_ALIGNMENT_TRAILING,
                ..Text::default()
            },
            color,
            1.,
        )
    };
    let Some(value) = indicator.value else {
        return text("—", c.p.muted);
    };
    let cx = (start + layout::METRIC_SPACING as f32 + end) / 2.;
    match indicator.style {
        Style::Value => text(&indicator.format_value(value), ink),
        Style::Bars => c.symbol(Symbol::MeterBars(value), cx, cy, 18., ink, 1.),
        Style::Ring => c.symbol_weighted(Symbol::MeterRing(value), cx, cy, 18., 2., ink, 1.),
        Style::Graph => c.draw_shapes(
            icon::history_trace(&indicator.history),
            cx,
            cy,
            27.,
            1.5,
            ink,
            1.,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Sample;
    use crate::platform::{Desktop, create_bars};

    #[test]
    #[ignore = "requires a Windows desktop with DirectComposition"]
    fn paints_and_reconfigures_monitors_on_hidden_bars() -> Result<()> {
        let _desktop = Desktop::init()?;
        let (top, bottom) = create_bars()?;
        let mut settings = crate::settings::Settings::default();
        let mut renderer = Renderer::new(top.0, bottom.0, false, &settings)?;
        let mut indicators = [
            Indicator::new(Metric::Cpu, Style::Bars),
            Indicator::new(Metric::Memory, Style::Ring),
            Indicator::new(Metric::Disk, Style::Graph),
            Indicator::new(Metric::GpuTemperature, Style::Value),
            Indicator::new(Metric::CpuTemperature, Style::Value),
        ];
        let lay_out = |renderer: &Renderer, metrics: &[Indicator]| {
            let measure = |text: &str, size, bold| renderer.measure(text, size, bold);
            StatusLayout::new((1200, 45), 144, 0, 0, &[], metrics, false, measure)
        };
        let mut bar = lay_out(&renderer, &indicators);
        renderer.draw_top(
            &bar,
            144,
            "DeskIsle",
            "12:00",
            &[],
            &[],
            None,
            None,
            &indicators,
            false,
            Some(0x301040),
        )?;
        for disk in [Some(0), Some(75), None, Some(93)] {
            let sample = Sample {
                cpu: Some(41),
                memory: Some(73),
                disk,
                gpu_temperature: Some(56),
                cpu_temperature: None,
            };
            for indicator in &mut indicators {
                indicator.update(sample, true);
            }
            renderer.draw_metrics(&bar, 144, &indicators)?;
        }
        use crate::settings::Key;
        settings.set(Key::Accent, 1);
        settings.set(Key::BoldFont, 1);
        settings.set(Key::FontFamily, 1);
        settings.set(Key::FontScale, 120);
        assert!(renderer.set_appearance(true, &settings));
        indicators[3].fahrenheit = true;
        indicators[0].configure(Style::Value, true);
        for family in 0..=2 {
            settings.set(Key::FontFamily, family);
            renderer.set_appearance(true, &settings);
            // The bars are laid out again whenever the font changes.
            bar = lay_out(&renderer, &indicators);
            for (metric, slot) in indicators.iter().zip(&bar.metrics) {
                if metric.style == Style::Value {
                    let maximum = if matches!(
                        metric.metric,
                        Metric::CpuTemperature | Metric::GpuTemperature
                    ) {
                        150
                    } else {
                        100
                    };
                    let text = metric.format_value(maximum);
                    let width = renderer.measure(&text, layout::METRIC_TEXT, false);
                    // The text may start a pixel after the symbol and ends half a gap
                    // before the slot does.
                    let around = layout::BAR_GAP + layout::BAR_ICON + 1;
                    let available = slot.width as f32 / 1.5 - around as f32;
                    assert!(
                        width <= available,
                        "{text} needs {width} DIPs with font {family}, available {available}"
                    );
                }
            }
        }
        settings.set(Key::TopBarBackground, 1);
        renderer.set_appearance(true, &settings);
        let frosted = top.set_top_backdrop(true, true);
        renderer.draw_top(
            &bar,
            144,
            "DeskIsle",
            "12:01",
            &[],
            &[],
            None,
            None,
            &indicators,
            frosted,
            None,
        )?;
        settings.set(Key::TopBarBackground, 2);
        renderer.set_appearance(true, &settings);
        top.set_top_backdrop(false, true);
        renderer.draw_top(
            &lay_out(&renderer, &[]),
            144,
            "DeskIsle",
            "12:01",
            &[],
            &[],
            None,
            None,
            &[],
            false,
            None,
        )?;
        use crate::ui::{
            layout::{StatusHit, SystemItem},
            system::Mark,
        };
        use windows::Win32::UI::WindowsAndMessaging::{IDI_INFORMATION, LoadIconW};
        let items = [
            SystemItem::InputMethod,
            SystemItem::InputLanguage,
            SystemItem::Search,
            SystemItem::Battery,
        ];
        let marks = [
            Mark::Icon {
                symbol: Symbol::Keyboard,
                dim: false,
            },
            Mark::Text("中"),
            Mark::Icon {
                symbol: Symbol::Search,
                dim: true,
            },
            // The bolt is cut out of the level where they overlap.
            Mark::Battery(Symbol::Battery {
                percent: Some(60),
                charging: true,
            }),
        ];
        let measure = |text: &str, size, bold| renderer.measure(text, size, bold);
        let bar = StatusLayout::new((1200, 45), 144, 1, 1, &items, &[], false, measure);
        let tray = [super::super::TrayImage {
            // LoadIconW returns a shared system icon.
            icon: Some(unsafe { LoadIconW(None, IDI_INFORMATION)? }),
            stamp: 1,
            needs_attention: false,
        }];
        // Gray tray icons on both grounds, then the expanded overflow list's tile.
        for (colored, dark, open) in [
            (0, true, None),
            (0, false, None),
            (1, false, Some(StatusHit::Overflow)),
        ] {
            settings.set(Key::ColorTrayIcons, colored);
            renderer.set_appearance(dark, &settings);
            renderer.draw_top(
                &bar,
                144,
                "DeskIsle",
                "12:01",
                &tray,
                &marks,
                open,
                None,
                &[],
                false,
                None,
            )?;
        }
        unsafe { renderer.device.WaitForCommitCompletion()? };
        renderer.check_device()
    }
}
