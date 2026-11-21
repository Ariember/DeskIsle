//! What the About card says about this computer: its name, model and form, and how the
//! processor, graphics, memory and Windows release are worded. The platform layer
//! gathers the raw values; anything it cannot read stays `None` and its row is left out.

/// Draws the card's illustration as a laptop or as a desktop display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Laptop,
    Desktop,
}

impl Form {
    /// The form for an SMBIOS chassis type, without its lock bit.
    pub fn from_chassis(chassis: u8) -> Self {
        match chassis & 0x7f {
            // Portable, laptop, notebook, hand held, sub notebook, tablet, convertible
            // and detachable.
            8 | 9 | 10 | 11 | 14 | 30 | 31 | 32 => Form::Laptop,
            _ => Form::Desktop,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub name: String,
    /// The nominal clock Windows records for the first processor.
    pub mhz: Option<u32>,
}

impl Cpu {
    /// "2.42 GHz Intel(R) Core(TM) i9-14900HX". Names that already state a clock keep it.
    pub fn text(&self) -> String {
        let name = collapse(&self.name);
        match self.mhz {
            Some(mhz) if mhz > 0 && !name.contains("Hz") => {
                format!("{:.2} GHz {name}", f64::from(mhz) / 1000.)
            }
            _ => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    /// Dedicated video memory in bytes.
    pub memory: u64,
}

const GIB: f64 = (1u64 << 30) as f64;
const MIB: u64 = 1 << 20;

impl Gpu {
    /// "NVIDIA GeForce RTX 4080 Laptop GPU 12 GB". The small carve-out an integrated
    /// adapter reports says nothing useful, so it is left out.
    pub fn text(&self) -> String {
        let name = collapse(&self.name);
        if self.memory as f64 >= GIB * 0.95 {
            format!("{name} {} GB", (self.memory as f64 / GIB).round())
        } else if self.memory >= 512 * MIB {
            format!("{name} {} MB", self.memory / MIB)
        } else {
            name
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    /// Physical memory available to Windows, in bytes.
    pub bytes: u64,
    /// Transfer rate of the installed modules, in MT/s, which vendors label MHz.
    pub speed: Option<u32>,
    /// The SMBIOS memory type code of the installed modules.
    pub kind: Option<u8>,
}

impl Memory {
    /// "31.6 GB 5600 MHz DDR5".
    pub fn text(&self) -> String {
        let gib = self.bytes as f64 / GIB;
        let size = if (gib - gib.round()).abs() < 0.05 {
            format!("{} GB", gib.round())
        } else {
            format!("{gib:.1} GB")
        };
        let mut parts = vec![size];
        if let Some(speed) = self.speed.filter(|&s| s > 0) {
            parts.push(format!("{speed} MHz"));
        }
        if let Some(kind) = self.kind.and_then(memory_kind) {
            parts.push(kind.to_owned());
        }
        parts.join(" ")
    }
}

/// The name of an SMBIOS memory type code.
pub fn memory_kind(code: u8) -> Option<&'static str> {
    Some(match code {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR3",
        0x1a => "DDR4",
        0x1b => "LPDDR",
        0x1c => "LPDDR2",
        0x1d => "LPDDR3",
        0x1e => "LPDDR4",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Os {
    /// `ProductName`, which still says Windows 10 on Windows 11.
    pub product: String,
    /// `EditionID`, such as `Professional`.
    pub edition: String,
    pub build: u32,
    /// The update build revision after the dot.
    pub revision: Option<u32>,
}

/// The Chinese name Windows shows for an edition.
fn edition_name(id: &str) -> Option<&'static str> {
    Some(match id {
        "Core" => "家庭版",
        "CoreN" => "家庭版 N",
        "CoreSingleLanguage" => "家庭单语言版",
        "CoreCountrySpecific" => "家庭中文版",
        "Professional" => "专业版",
        "ProfessionalN" => "专业版 N",
        "ProfessionalWorkstation" => "专业工作站版",
        "ProfessionalEducation" => "专业教育版",
        "Education" => "教育版",
        "Enterprise" => "企业版",
        "EnterpriseS" => "企业版 LTSC",
        "IoTEnterprise" => "IoT 企业版",
        "IoTEnterpriseS" => "IoT 企业版 LTSC",
        _ => return None,
    })
}

impl Os {
    /// "Windows 11 专业版 26200.9711". Windows 11 is told apart by its build number,
    /// since its product name was never updated.
    pub fn text(&self) -> String {
        let product = collapse(&self.product);
        let base = if product.contains("Server") || self.build < 10240 {
            product
        } else {
            let family = if self.build >= 22000 {
                "Windows 11"
            } else {
                "Windows 10"
            };
            match edition_name(&self.edition) {
                Some(edition) => format!("{family} {edition}"),
                None => product.replacen("Windows 10", family, 1),
            }
        };
        match self.revision {
            Some(revision) => format!("{base} {}.{revision}", self.build),
            None => format!("{base} {}", self.build),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub name: String,
    /// The model the firmware reports, when it reports a real one.
    pub model: Option<String>,
    pub form: Form,
    pub cpu: Option<Cpu>,
    pub gpu: Option<Gpu>,
    pub memory: Option<Memory>,
    pub os: Option<Os>,
}

impl Machine {
    /// The card's labeled rows, in display order, skipping facts that are unknown.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        [
            ("处理器", self.cpu.as_ref().map(Cpu::text)),
            ("图形卡", self.gpu.as_ref().map(Gpu::text)),
            ("内存", self.memory.as_ref().map(Memory::text)),
            ("版本", self.os.as_ref().map(Os::text)),
        ]
        .into_iter()
        .filter_map(|(label, text)| Some((label, text.filter(|t| !t.is_empty())?)))
        .collect()
    }
}

/// Firmware fills unset fields with placeholders such as "To be filled by O.E.M.".
pub fn real_model(text: &str) -> Option<String> {
    let text = collapse(text);
    let lower = text.to_ascii_lowercase();
    let placeholder = [
        "to be filled",
        "system product name",
        "default string",
        "not applicable",
    ]
    .iter()
    .any(|p| lower.contains(p));
    (!text.is_empty() && !placeholder).then_some(text)
}

/// Trims the text and joins runs of whitespace, which registry names often pad with.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_word_each_fact_like_the_about_card() {
        let machine = Machine {
            name: "DESKTOP-CAT6HGQ".into(),
            model: real_model("ROG Strix SCAR 18 G834JZR_G834JZR"),
            form: Form::from_chassis(10),
            cpu: Some(Cpu {
                name: "Intel(R) Core(TM) i9-14900HX  ".into(),
                mhz: Some(2419),
            }),
            gpu: Some(Gpu {
                name: "NVIDIA GeForce RTX 4080 Laptop GPU".into(),
                memory: 12_282 * MIB,
            }),
            memory: Some(Memory {
                bytes: 33_954_418_688,
                speed: Some(5600),
                kind: Some(0x22),
            }),
            os: Some(Os {
                product: "Windows 10 Pro".into(),
                edition: "Professional".into(),
                build: 26200,
                revision: Some(9711),
            }),
        };
        assert_eq!(machine.form, Form::Laptop);
        assert_eq!(
            machine.rows(),
            [
                ("处理器", "2.42 GHz Intel(R) Core(TM) i9-14900HX".to_owned()),
                (
                    "图形卡",
                    "NVIDIA GeForce RTX 4080 Laptop GPU 12 GB".to_owned()
                ),
                ("内存", "31.6 GB 5600 MHz DDR5".to_owned()),
                ("版本", "Windows 11 专业版 26200.9711".to_owned()),
            ]
        );
    }

    #[test]
    fn unknown_facts_drop_their_rows_and_details() {
        let machine = Machine {
            name: "PC".into(),
            model: real_model("To be filled by O.E.M."),
            form: Form::from_chassis(3),
            cpu: Some(Cpu {
                name: "Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz".into(),
                mhz: Some(3192),
            }),
            gpu: Some(Gpu {
                name: "Intel(R) UHD Graphics 630".into(),
                memory: 128 * MIB,
            }),
            memory: Some(Memory {
                bytes: 16 << 30,
                speed: None,
                kind: Some(0x02),
            }),
            os: None,
        };
        assert_eq!(machine.model, None);
        assert_eq!(machine.form, Form::Desktop);
        assert_eq!(
            machine.rows(),
            [
                (
                    "处理器",
                    "Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz".to_owned()
                ),
                ("图形卡", "Intel(R) UHD Graphics 630".to_owned()),
                ("内存", "16 GB".to_owned()),
            ]
        );
    }

    #[test]
    fn windows_release_follows_the_build_not_the_product_name() {
        let os = |product: &str, edition: &str, build| Os {
            product: product.into(),
            edition: edition.into(),
            build,
            revision: None,
        };
        assert_eq!(
            os("Windows 10 Home", "Core", 19045).text(),
            "Windows 10 家庭版 19045"
        );
        assert_eq!(
            os("Windows 10 Pro for Workstations", "Unknown", 22631).text(),
            "Windows 11 Pro for Workstations 22631"
        );
        assert_eq!(
            os("Windows Server 2022 Datacenter", "ServerDatacenter", 20348).text(),
            "Windows Server 2022 Datacenter 20348"
        );
    }
}
