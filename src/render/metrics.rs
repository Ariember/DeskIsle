use windows::Win32::Graphics::{
    DirectComposition::IDCompositionVisual2, DirectWrite::IDWriteFactory,
};
use windows::core::Result;

use super::canvas::{Canvas, Palette, paint};
use super::{Dimensions, Renderer};
use crate::metrics::{Indicator, Metric, Style};
use crate::ui::icon::{self, Symbol};
use crate::ui::layout::StatusLayout;

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
        if self.update_metrics(layout, dpi, metrics)? {
            unsafe { self.device.Commit()? };
        }
        Ok(())
    }

    pub(super) fn update_metrics(
        &mut self,
        layout: &StatusLayout,
        dpi: u32,
        readings: &[Indicator],
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
            && (layer.size, layer.palette) == (size, self.palette)
            && layer.readings == readings
        {
            if first.x == layer.left {
                return Ok(false);
            }
            unsafe { layer.visual.SetOffsetX2(first.x as f32)? };
            layer.left = first.x;
            return Ok(true);
        }
        let surface = paint(&self.device, self.palette, size.0, size.1, dpi, |c| {
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
                palette: self.palette,
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
    let (symbol, unit) = match indicator.metric {
        Metric::Cpu => (Symbol::Chip, "%"),
        Metric::Memory => (Symbol::Memory, "%"),
        Metric::Disk => (Symbol::Disk, "%"),
        Metric::CpuTemperature => (Symbol::Thermometer, "°C"),
        Metric::GpuTemperature => (Symbol::GraphicsCard, "°C"),
    };
    let ink = if indicator.highlight {
        c.p.danger
    } else {
        c.p.ink
    };
    let cy = height / 2.;
    c.symbol(symbol, left + 12., cy, 17., ink, 1.)?;
    let text_rect = c.rect(left + 23., 0., right - 2., height);
    let Some(value) = indicator.value else {
        return c.text(write, "—", text_rect, 12., false, c.p.muted);
    };
    let cx = (left + 22. + right - 3.) / 2.;
    match indicator.style {
        Style::Value => c.text(write, &format!("{value}{unit}"), text_rect, 12., false, ink),
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
        let mut renderer = Renderer::new(top.0, bottom.0, false)?;
        let mut indicators = [
            Indicator::new(Metric::Cpu, Style::Bars),
            Indicator::new(Metric::Memory, Style::Ring),
            Indicator::new(Metric::Disk, Style::Graph),
            Indicator::new(Metric::GpuTemperature, Style::Value),
            Indicator::new(Metric::CpuTemperature, Style::Value),
        ];
        let layout = |metrics: &[Indicator]| StatusLayout::new(1200, 45, 144, 0, 0, &[], metrics);
        let mut bar = layout(&indicators);
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
        renderer.set_dark(true);
        indicators[0].configure(Style::Value, true);
        bar = layout(&indicators);
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
        )?;
        renderer.draw_top(
            &layout(&[]),
            144,
            "DeskIsle",
            "12:01",
            &[],
            &[],
            None,
            None,
            &[],
        )?;
        unsafe { renderer.device.WaitForCommitCompletion()? };
        renderer.check_device()
    }
}
