//! System-wide readings. Missing values include the first sample of a rate counter.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    pub cpu: bool,
    pub memory: bool,
    pub disk: bool,
    pub cpu_temperature: bool,
    pub gpu_temperature: bool,
}

impl FromIterator<Metric> for Sources {
    fn from_iter<T: IntoIterator<Item = Metric>>(metrics: T) -> Self {
        let mut sources = Self::default();
        for metric in metrics {
            match metric {
                Metric::Cpu => sources.cpu = true,
                Metric::Memory => sources.memory = true,
                Metric::Disk => sources.disk = true,
                Metric::CpuTemperature => sources.cpu_temperature = true,
                Metric::GpuTemperature => sources.gpu_temperature = true,
            }
        }
        sources
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub disk: Option<u8>,
    pub cpu_temperature: Option<u8>,
    pub gpu_temperature: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Memory,
    Cpu,
    Disk,
    CpuTemperature,
    GpuTemperature,
}

impl Metric {
    pub const ALL: [Self; 5] = [
        Self::Memory,
        Self::Cpu,
        Self::Disk,
        Self::CpuTemperature,
        Self::GpuTemperature,
    ];

    pub fn enabled(self, sources: Sources) -> bool {
        match self {
            Self::Cpu => sources.cpu,
            Self::Memory => sources.memory,
            Self::Disk => sources.disk,
            Self::CpuTemperature => sources.cpu_temperature,
            Self::GpuTemperature => sources.gpu_temperature,
        }
    }

    fn reading(self, sample: Sample) -> Option<u8> {
        match self {
            Self::Cpu => sample.cpu,
            Self::Memory => sample.memory,
            Self::Disk => sample.disk,
            Self::CpuTemperature => sample.cpu_temperature,
            Self::GpuTemperature => sample.gpu_temperature,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bars,
    Ring,
    Graph,
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indicator {
    pub metric: Metric,
    pub style: Style,
    pub value: Option<u8>,
    pub history: Vec<Option<u8>>,
    pub highlight: bool,
    /// Display preference only. Samples and history always use the sensor's Celsius value.
    pub fahrenheit: bool,
}

impl Indicator {
    pub fn new(metric: Metric, style: Style) -> Self {
        Self {
            metric,
            style,
            value: None,
            history: Vec::new(),
            highlight: false,
            fahrenheit: false,
        }
    }

    pub fn format_value(&self, value: u8) -> String {
        match self.metric {
            Metric::CpuTemperature | Metric::GpuTemperature if self.fahrenheit => {
                let degrees = (u16::from(value) * 9 + 2) / 5 + 32;
                format!("{degrees}°F")
            }
            Metric::CpuTemperature | Metric::GpuTemperature => format!("{value}°C"),
            Metric::Cpu | Metric::Memory | Metric::Disk => format!("{value}%"),
        }
    }

    pub fn configure(&mut self, style: Style, alert: bool) {
        if self.style != Style::Graph && style == Style::Graph && self.value.is_some() {
            self.history.push(self.value);
        }
        self.style = style;
        if style != Style::Graph {
            self.history.clear();
        }
        self.highlight = alert
            && matches!(self.metric, Metric::Cpu | Metric::Memory | Metric::Disk)
            && self.value.is_some_and(|v| v >= 90);
    }

    pub fn update(&mut self, sample: Sample, alert: bool) {
        self.value = self.metric.reading(sample);
        if self.style == Style::Graph {
            // Thirty two-second samples cover one minute. None breaks the line.
            if self.history.len() == 30 {
                self.history.remove(0);
            }
            self.history.push(self.value);
        }
        self.configure(self.style, alert);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_keeps_one_minute_including_gaps_and_highlights_only_valid_loads() {
        let mut cpu = Indicator::new(Metric::Cpu, Style::Graph);
        for n in 0..31 {
            cpu.update(
                Sample {
                    cpu: Some(n),
                    ..Sample::default()
                },
                true,
            );
        }
        cpu.update(Sample::default(), true);
        assert_eq!(cpu.history.len(), 30);
        assert_eq!(cpu.history[0], Some(2));
        assert_eq!(cpu.history.last(), Some(&None));
        assert!(!cpu.highlight);
        cpu.update(
            Sample {
                cpu: Some(90),
                ..Sample::default()
            },
            true,
        );
        assert!(cpu.highlight);
        cpu.configure(Style::Value, false);
        assert!(!cpu.highlight && cpu.history.is_empty());
        assert_eq!(cpu.value, Some(90));
        let mut temperature = Indicator::new(Metric::GpuTemperature, Style::Value);
        temperature.update(
            Sample {
                gpu_temperature: Some(90),
                ..Sample::default()
            },
            true,
        );
        assert!(!temperature.highlight);
        assert_eq!(temperature.format_value(23), "23°C");
        temperature.fahrenheit = true;
        assert_eq!(temperature.format_value(23), "73°F");
        assert_eq!(temperature.format_value(150), "302°F");
    }
}
