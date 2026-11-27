use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CardState {
    #[default]
    Ready,
    Setup,
    Error,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CardTone {
    #[default]
    Accent,
    Neutral,
    Good,
    Warn,
    Danger,
    Purple,
    Teal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetricItem {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub sub: String,
    #[serde(default)]
    pub tone: CardTone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SegmentItem {
    pub label: String,
    pub share: u8,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub tone: CardTone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeterItem {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub detail: String,
    pub percent: u8,
    #[serde(default)]
    pub tone: CardTone,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum CardBlock {
    Metrics {
        #[serde(default)]
        title: String,
        items: Vec<MetricItem>,
    },
    Segments {
        #[serde(default)]
        title: String,
        #[serde(default)]
        note: String,
        items: Vec<SegmentItem>,
    },
    Meters {
        #[serde(default)]
        title: String,
        items: Vec<MeterItem>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Card {
    /// Selected by the host from the installed plugin ID.
    #[serde(skip)]
    pub white_surface: bool,
    pub title: String,
    pub value: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub state: CardState,
    #[serde(default)]
    pub badge: String,
    #[serde(default)]
    pub blocks: Vec<CardBlock>,
    #[serde(default)]
    pub footer: String,
}

impl Card {
    pub(crate) fn simple(
        title: impl Into<String>,
        value: impl Into<String>,
        detail: impl Into<String>,
        state: CardState,
    ) -> Self {
        Self {
            white_surface: false,
            title: title.into(),
            value: value.into(),
            detail: detail.into(),
            state,
            badge: String::new(),
            blocks: Vec::new(),
            footer: String::new(),
        }
    }

    pub(crate) fn is_rich(&self) -> bool {
        !self.blocks.is_empty() || !self.footer.is_empty() || !self.badge.is_empty()
    }
}
