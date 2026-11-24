//! Versioned snapshot protocol for out-of-process DeskIsle plugins.

use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::cards::Card;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug)]
pub(crate) struct Publication {
    pub name: String,
    pub version: Version,
    pub contributions: Vec<Contribution>,
}

#[derive(Debug)]
pub(crate) enum Contribution {
    DesktopCards(Vec<Card>),
}

struct ExtensionPoint {
    name: &'static str,
    version: u32,
    decode: fn(Value) -> Result<Contribution, String>,
}

// Each entry advertises and decodes one supported point/version pair.
const EXTENSIONS: &[ExtensionPoint] = &[ExtensionPoint {
    name: "desktop.cards",
    version: 1,
    decode: decode_cards,
}];

pub(crate) fn capabilities() -> Value {
    let mut points: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for extension in EXTENSIONS {
        points
            .entry(extension.name)
            .or_default()
            .push(extension.version);
    }
    json!({ "protocol": PROTOCOL_VERSION, "points": points })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    protocol: u32,
    name: String,
    version: String,
    contributions: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    point: String,
    version: u32,
    #[serde(default = "required_by_default")]
    required: bool,
    data: Value,
}

fn required_by_default() -> bool {
    true
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Publication, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("单个插件文件最多 64 KiB".into());
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let document: Document = serde_json::from_slice(bytes).map_err(|e| {
        format!(
            "JSON 不符合插件规范（第 {} 行，第 {} 列）",
            e.line(),
            e.column()
        )
    })?;
    if document.protocol != PROTOCOL_VERSION {
        return Err(format!("此版本仅支持插件协议 v{PROTOCOL_VERSION}"));
    }
    if !valid_text(&document.name, 64, true) {
        return Err("插件 name 不符合文本规范".into());
    }
    let version = Version::parse(&document.version).map_err(|_| "插件 version 必须是语义化版本")?;
    if document.contributions.len() > 16 {
        return Err("单个插件最多声明 16 个扩展点".into());
    }
    let mut seen = BTreeSet::new();
    let mut contributions = Vec::new();
    for entry in document.contributions {
        if !valid_point(&entry.point) || entry.version == 0 {
            return Err("扩展点名称或版本无效".into());
        }
        let extension = EXTENSIONS
            .iter()
            .find(|ext| ext.name == entry.point && ext.version == entry.version);
        if !seen.insert(entry.point) {
            return Err("同一插件重复声明了扩展点".into());
        }
        if let Some(extension) = extension {
            contributions.push((extension.decode)(entry.data)?);
        } else if entry.required {
            return Err("宿主不支持此插件要求的扩展点或版本".into());
        }
    }
    Ok(Publication {
        name: document.name,
        version,
        contributions,
    })
}

fn valid_point(point: &str) -> bool {
    point.len() <= 64
        && point.contains('.')
        && point.split('.').all(|part| {
            part.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

fn valid_text(text: &str, limit: usize, required: bool) -> bool {
    (!required || !text.trim().is_empty())
        && text.chars().count() <= limit
        && !text
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

fn decode_cards(value: Value) -> Result<Contribution, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CardData {
        cards: Vec<Card>,
    }
    let data: CardData = serde_json::from_value(value).map_err(|_| "desktop.cards 数据格式无效")?;
    if data.cards.len() > 16 {
        return Err("单个插件最多发布 16 张卡片".into());
    }
    for (i, card) in data.cards.iter().enumerate() {
        for (name, value, limit, required) in [
            ("title", &card.title, 64, true),
            ("value", &card.value, 64, true),
            ("detail", &card.detail, 160, false),
        ] {
            if !valid_text(value, limit, required) {
                return Err(format!("第 {} 张卡片的 {name} 不符合文本规范", i + 1));
            }
        }
    }
    Ok(Contribution::DesktopCards(data.cards))
}

pub(crate) fn valid_plugin_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 64
        || id.starts_with('-')
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || matches!(id, "con" | "prn" | "aux" | "nul")
    {
        return false;
    }
    let device = id.strip_prefix("com").or_else(|| id.strip_prefix("lpt"));
    !device.is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::CardState;

    fn document(entries: Value) -> Value {
        json!({"protocol":1,"name":"Test","version":"1.2.3-beta.1","contributions":entries})
    }

    fn parse_entries(entries: Value) -> Result<Publication, String> {
        parse(&serde_json::to_vec(&document(entries)).unwrap())
    }

    fn cards_entry(cards: Value) -> Value {
        json!({"point":"desktop.cards","version":1,"data":{"cards":cards}})
    }

    #[test]
    fn validates_metadata_and_discovers_supported_contracts() {
        let plugin = parse_entries(json!([])).unwrap();
        assert_eq!(plugin.name, "Test");
        assert_eq!(plugin.version, Version::parse("1.2.3-beta.1").unwrap());
        assert!(plugin.contributions.is_empty());
        let advertised = capabilities();
        assert_eq!(
            advertised,
            json!({"protocol":1,"points":{"desktop.cards":[1]}})
        );
        for (point, versions) in advertised["points"].as_object().unwrap() {
            for version in versions.as_array().unwrap() {
                let entry = json!({"point":point,"version":version,"data":{"cards":[]}});
                assert_eq!(
                    parse_entries(json!([entry])).unwrap().contributions.len(),
                    1
                );
            }
        }
        for (key, value) in [
            ("protocol", json!(2)),
            ("name", json!("  ")),
            ("name", json!("x\u{2029}y")),
            ("version", json!("1.0")),
            ("future-field", json!(true)),
        ] {
            let mut invalid = document(json!([]));
            invalid[key] = value;
            assert!(parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        assert!(parse(&vec![b' '; MAX_DOCUMENT_BYTES + 1]).is_err());
    }

    #[test]
    fn negotiates_only_supported_points_and_rejects_duplicate_declarations() {
        let cards = cards_entry(json!([]));
        for (name, version) in [("vendor.some-point", 1), ("desktop.cards", 2)] {
            let mut other = json!({"point":name,"version":version,"data":42});
            assert!(parse_entries(json!([other])).is_err());
            other["required"] = json!(false);
            assert!(
                parse_entries(json!([other]))
                    .unwrap()
                    .contributions
                    .is_empty()
            );
            assert!(parse_entries(json!([other, other])).is_err());
            if name == "desktop.cards" {
                assert!(parse_entries(json!([other, cards])).is_err());
            } else {
                assert_eq!(
                    parse_entries(json!([other, cards]))
                        .unwrap()
                        .contributions
                        .len(),
                    1
                );
            }
        }
        for name in [
            "upper.Case",
            "single",
            "vendor..empty",
            "app.0",
            "app.-action",
            "app.a_b",
        ] {
            assert!(
                parse_entries(json!([{"point":name,"version":1,"required":false,"data":{}}]))
                    .is_err()
            );
        }
        for version in [json!(0), json!(-1), json!(4294967296u64)] {
            assert!(
                parse_entries(
                    json!([{"point":"test.future","version":version,"required":false,"data":{}}])
                )
                .is_err()
            );
        }
        assert!(
            parse_entries(json!([{"point":"future.ext","version":1,"required":false}])).is_err()
        );
        let points: Vec<_> = (0..17).map(|n| json!({"point":format!("vendor.v{n}"),"version":1,"data":null,"required":false})).collect();
        assert!(parse_entries(json!(points)).is_err());
    }

    #[test]
    fn validates_known_contributions_even_when_optional() {
        let base = json!({"title":"状态","value":"42"});
        let plugin = parse_entries(json!([cards_entry(json!([base]))])).unwrap();
        let Contribution::DesktopCards(cards) = &plugin.contributions[0];
        assert_eq!(
            cards,
            &[Card {
                title: "状态".into(),
                value: "42".into(),
                detail: String::new(),
                state: CardState::Ready,
            }]
        );
        for (field, input) in [
            ("title", json!("  ")),
            ("value", json!(42)),
            ("value", json!("界".repeat(65))),
            ("detail", json!("foo\nbar")),
            ("state", json!("private-payload")),
            ("private-payload", json!(true)),
        ] {
            let mut invalid = base.clone();
            invalid[field] = input;
            let mut optional = cards_entry(json!([invalid]));
            optional["required"] = json!(false);
            assert!(
                !parse_entries(json!([optional]))
                    .unwrap_err()
                    .contains("private-payload")
            );
        }
        assert!(parse_entries(json!([cards_entry(json!(vec![base; 17]))])).is_err());
        let valid = json!({"title":"s","value":"v","detail":"界".repeat(160),"state":"setup"});
        assert!(parse_entries(json!([cards_entry(json!([valid]))])).is_ok());
    }

    #[test]
    fn id_cannot_resolve_to_a_path_device_or_case_alias() {
        for invalid in [
            "", ".", "..", "foo/bar", "foo\\bar", "F", "f.", "f ", "-f", "con", "aux", "lpt1",
            "com9", "nul:", "a_b", "令牌",
        ] {
            assert!(!valid_plugin_id(invalid), "{invalid}");
        }
        assert!(!valid_plugin_id(&"a".repeat(65)));
        for valid in ["0", "plug-in", "com10", &"z".repeat(64)] {
            assert!(valid_plugin_id(valid), "{valid}");
        }
    }
}
