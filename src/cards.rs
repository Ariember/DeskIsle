use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CardState {
    #[default]
    Ready,
    Setup,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Card {
    pub title: String,
    pub value: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub state: CardState,
}
