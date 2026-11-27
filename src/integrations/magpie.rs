//! Read a local Magpie gateway snapshot for the desktop card.
use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, de::DeserializeOwned};
use ureq::Agent;

use crate::cards::{Card, CardBlock, CardState, CardTone, MeterItem, MetricItem};

const TITLE: &str = "Magpie 数据面板";
const DEFAULT_PORT: u16 = 3425;

fn configured_port() -> Option<u16> {
    #[derive(Deserialize)]
    struct GatewaySettings {
        port: u16,
    }
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("magpie/settings.json"))
        .filter(|p| p.is_file())
        .or_else(|| Some(std::env::home_dir()?.join(".config/magpie/settings.json")))?;
    let settings: GatewaySettings =
        serde_json::from_reader(std::fs::File::open(path).ok()?).ok()?;
    (settings.port != 0).then_some(settings.port)
}

pub(crate) fn read_cards() -> Vec<Card> {
    vec![read_card(configured_port().unwrap_or(DEFAULT_PORT))]
}

fn get<T: DeserializeOwned>(agent: &Agent, url: String) -> Result<T, ()> {
    let mut response = agent
        .get(url)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| ())?;
    if response.status() != 200 {
        return Err(());
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
}

fn read_card(port: u16) -> Card {
    let base = format!("http://127.0.0.1:{port}");
    let agent: Agent = Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let Ok(usage) = get::<Usage>(&agent, format!("{base}/v1/magpie/usage?period=today")) else {
        let mut card = Card::simple(
            TITLE,
            "未连接",
            format!("请确认 Magpie 已启动且用量接口可用（{base}）"),
            CardState::Error,
        );
        card.badge = format!("异常 · {}", Local::now().format("%H:%M"));
        card.footer = "确认 Magpie 已启动后将自动恢复同步".into();
        return card;
    };
    let quotas = get::<QuotaList>(&agent, format!("{base}/v1/magpie/quotas"));
    build_card(usage, quotas, Local::now())
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Tokens {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl Tokens {
    fn prompt(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
    fn total(&self) -> u64 {
        self.prompt().saturating_add(self.output)
    }
}

#[derive(Deserialize)]
struct Usage {
    calls: u64,
    #[serde(default)]
    errors: u64,
    #[serde(rename = "sessionCount", default)]
    sessions: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    unpriced: u64,
    #[serde(flatten)]
    tokens: Tokens,
    models: Option<Vec<Model>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Model {
    name: String,
    model: String,
    id: String,
    calls: u64,
    #[serde(flatten)]
    tokens: Tokens,
}

#[derive(Deserialize)]
struct QuotaList {
    // Go encodes an empty nil slice as null.
    #[serde(deserialize_with = "deserialize_accounts")]
    data: Vec<Account>,
}

fn deserialize_accounts<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<Account>, D::Error> {
    Ok(Option::<Vec<Account>>::deserialize(de)?.unwrap_or_default())
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Account {
    name: String,
    provider: String,
    #[serde(rename = "asOf")]
    as_of: String,
    windows: Option<Vec<Window>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Window {
    remaining: Option<f64>,
    display: String,
    text: bool,
    #[serde(rename = "resetsAt")]
    resets_at: String,
}

fn token_count(n: u64) -> String {
    let count = n as f64;
    match n {
        0..1_000 => n.to_string(),
        1_000..100_000 => format!("{:.1}K", count / 1e3),
        100_000..1_000_000 => format!("{:.0}K", count / 1e3),
        1_000_000..10_000_000 => format!("{:.1}M", count / 1e6),
        10_000_000..1_000_000_000 => format!("{:.0}M", count / 1e6),
        _ => format!("{:.2}B", count / 1e9),
    }
}

fn percent(pct: f64) -> String {
    if pct > 0.0 && pct < 1.0 {
        "<1%".into()
    } else {
        format!("{pct:.0}%")
    }
}

fn label(text: &str, limit: usize) -> String {
    let clean: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if clean.chars().count() <= limit {
        return clean;
    }
    clean.chars().take(limit - 1).chain(['…']).collect()
}

fn model_label(model: &Model) -> String {
    let name = [&model.name, &model.model, &model.id]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .map_or("unknown", |s| s.trim());
    let mut name = name.rsplit('/').next().unwrap_or(name);
    if name.chars().count() > 9 {
        name = ["claude-", "gemini-", "deepseek-", "qwen-"]
            .iter()
            .find_map(|prefix| name.strip_prefix(prefix))
            .unwrap_or(name);
    }
    label(name, 9)
}

fn usage_metrics(usage: &Usage) -> CardBlock {
    let t = &usage.tokens;
    let (name, value, detail) = if t.cache_read > 0 {
        let mut detail = format!("读 {}", token_count(t.cache_read));
        if t.cache_write > 0 {
            detail.push_str(&format!(" · 写 {}", token_count(t.cache_write)));
        } else if usage.reasoning > 0 {
            detail.push_str(&format!(" · 推 {}", token_count(usage.reasoning)));
        }
        (
            "缓存命中",
            percent(100.0 * t.cache_read as f64 / t.prompt() as f64),
            detail,
        )
    } else if usage.reasoning > 0 {
        (
            "推理 Token",
            token_count(usage.reasoning),
            format!("缓写 {}", token_count(t.cache_write)),
        )
    } else {
        ("缓存命中", "0%".into(), "读 0 · 写 0".into())
    };
    CardBlock::Metrics {
        title: "Token 用量".into(),
        items: vec![
            MetricItem {
                label: "总 Token".into(),
                value: token_count(t.total()),
                sub: format!("入 {} · 出 {}", token_count(t.input), token_count(t.output)),
                tone: CardTone::Neutral,
            },
            MetricItem {
                label: name.into(),
                value,
                sub: detail,
                tone: CardTone::Neutral,
            },
        ],
    }
}

fn model_block(usage: &Usage, mut models: Vec<Model>) -> CardBlock {
    let tokens = usage.tokens.total();
    let weight = |m: &Model| {
        if tokens > 0 {
            m.tokens.total()
        } else {
            m.calls
        }
    };
    let denominator = if tokens > 0 { tokens } else { usage.calls };
    models.sort_by_key(|m| Reverse((weight(m), m.calls)));
    let row = |name, weight: u64| {
        let pct = (100.0 * weight as f64 / denominator as f64).min(100.0);
        MeterItem {
            label: name,
            value: percent(pct),
            percent: if weight > 0 {
                pct.round().max(1.0) as u8
            } else {
                0
            },
            detail: String::new(),
            tone: CardTone::Neutral,
        }
    };
    let count = models.len();
    let shown = if count > 3 { 2 } else { count };
    let mut included = 0u64;
    let mut items = Vec::new();
    for m in &models[..shown] {
        let n = weight(m);
        included = included.saturating_add(n);
        items.push(row(model_label(m), n));
    }
    if count > shown {
        items.push(row("其他".into(), denominator.saturating_sub(included)));
    }
    CardBlock::Meters {
        title: format!("模型占比 · 共 {count} 个"),
        items,
    }
}

fn reset_detail(timestamp: &str, now: DateTime<Local>) -> String {
    let Ok(reset) = DateTime::parse_from_rfc3339(timestamp) else {
        return String::new();
    };
    let seconds = reset.signed_duration_since(now).num_milliseconds() as f64 / 1000.0;
    if seconds <= 60.0 {
        return "即将重置".into();
    }
    if seconds < 3600.0 {
        return format!("{}分重置", (seconds / 60.0).ceil());
    }
    if seconds < 86400.0 {
        let hours = seconds as i64 / 3600;
        let minutes = seconds as i64 / 60 % 60;
        if hours < 6 && minutes > 0 {
            return format!("{hours}时{minutes}分重置");
        }
        return format!("{:.0}时重置", seconds / 3600.0);
    }
    format!("{:.0}天重置", seconds / 86400.0)
}

fn account_row(account: Account, now: DateTime<Local>) -> Option<MeterItem> {
    let (remaining, window) = account
        .windows?
        .into_iter()
        .filter(|w| !w.text && !w.display.eq_ignore_ascii_case("unknown"))
        .filter_map(|w| Some((w.remaining?, w)))
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    let percent = remaining.round().clamp(0.0, 100.0) as u8;
    let detail = if account.as_of.is_empty() {
        reset_detail(&window.resets_at, now)
    } else {
        DateTime::parse_from_rfc3339(&account.as_of).map_or_else(
            |_| "缓存读数".into(),
            |t| format!("缓存 {}", t.with_timezone(&Local).format("%m-%d %H:%M")),
        )
    };
    let name = [account.name.as_str(), &account.provider]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or("订阅");
    Some(MeterItem {
        label: label(name, 32),
        value: if percent == 0 {
            "已用尽".into()
        } else {
            format!("剩 {percent}%")
        },
        percent,
        detail,
        tone: CardTone::Neutral,
    })
}

fn quota_block(quotas: Result<QuotaList, ()>, now: DateTime<Local>) -> CardBlock {
    let mut items = Vec::new();
    let failed = quotas.is_err();
    if let Ok(quotas) = quotas {
        items.extend(quotas.data.into_iter().filter_map(|q| account_row(q, now)));
        items.sort_by(|a, b| (a.percent, &a.label).cmp(&(b.percent, &b.label)));
        items.truncate(4);
    }
    if items.is_empty() {
        let (label, value, detail) = if failed {
            ("配额接口异常", "读取失败", "请确认 Magpie 配额接口可用")
        } else {
            ("订阅账号", "暂无配额", "未检测到订阅配额")
        };
        items.push(MeterItem {
            label: label.into(),
            value: value.into(),
            detail: detail.into(),
            percent: 0,
            tone: CardTone::Neutral,
        });
    }
    CardBlock::Meters {
        title: "订阅配额".into(),
        items,
    }
}

fn build_card(mut usage: Usage, quotas: Result<QuotaList, ()>, now: DateTime<Local>) -> Card {
    let mut card = Card::simple(
        TITLE,
        "今天还没有请求",
        "统计网关调用与 Agent 会话日志。",
        CardState::Ready,
    );
    card.badge = format!("今日 · {}", now.format("%H:%M"));
    if usage.calls > 0 {
        let cost = usage.cost;
        card.value = if cost <= 0.0 {
            format!("{} 次调用", usage.calls)
        } else if cost < 0.01 {
            "<$0.01".into()
        } else if cost < 100.0 {
            format!("${cost:.2}")
        } else if cost < 1000.0 {
            format!("${cost:.1}")
        } else {
            format!("${cost:.0}")
        };
        let mut parts = vec![format!("{} 次请求", usage.calls)];
        if usage.sessions > 0 {
            parts.push(format!("{} 会话", usage.sessions));
        }
        if usage.errors > 0 {
            parts.push(format!("{} 错", usage.errors));
        }
        if cost <= 0.0 && usage.unpriced > 0 {
            parts.push("含未计价".into());
        }
        card.detail = parts.join(" · ");
        card.blocks.push(usage_metrics(&usage));
        if let Some(models) = usage.models.take().filter(|v| !v.is_empty()) {
            card.blocks.push(model_block(&usage, models));
        }
    }
    card.blocks.push(quota_block(quotas, now));
    card
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn polling_reads_both_endpoints_again_after_failures() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let usage = "/v1/magpie/usage?period=today";
            let quota = "/v1/magpie/quotas";
            for (expected, status, body) in [
                (usage, 500, ""),
                (usage, 200, r#"{"calls":2}"#),
                (quota, 503, ""),
                (usage, 200, r#"{"calls":3}"#),
                (
                    quota,
                    200,
                    r#"{"data":[{"provider":"codex","windows":[{"remaining":42}]}]}"#,
                ),
            ] {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(err)
                            if err.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(err) => panic!("expected {expected}: {err}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line.split_whitespace().nth(1), Some(expected));
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                write!(stream, "HTTP/1.1 {status} Test\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        assert_eq!(read_card(port).state, CardState::Error);
        let partial = read_card(port);
        assert_eq!(partial.value, "2 次调用");
        let CardBlock::Meters { items, .. } = partial.blocks.last().unwrap() else {
            panic!()
        };
        assert_eq!(items[0].value, "读取失败");
        let fresh = read_card(port);
        assert_eq!(fresh.value, "3 次调用");
        let CardBlock::Meters { items, .. } = fresh.blocks.last().unwrap() else {
            panic!()
        };
        assert_eq!(items[0].percent, 42);
        server.join().unwrap();
    }

    #[test]
    fn active_snapshot_ranks_models_and_numeric_quota_windows() {
        let now = DateTime::parse_from_rfc3339("2026-11-25T10:15:00+08:00")
            .unwrap()
            .with_timezone(&Local);
        let usage = serde_json::from_value(json!({
        "calls": 4, "input": 100, "cache_read": 900, "output": 1000, "cost": 158.06,
        "errors": 1, "sessionCount": 2, "models": [
            {"id":"p/test","calls":1,"input":100},
            {"name":"claude-opus-5-5","calls":1,"output":1000},
            {"model":"two","calls":1,"cache_read":800},
            {"name":"four","calls":1,"cache_read":100}
        ]}))
        .unwrap();
        let quotas = serde_json::from_value(json!({"data":[
            {"name":"二", "asOf":"2026-11-25T10:15:00+08:00","windows":[{"remaining":35}]},
            {"name":"一","windows":[{"remaining":2,"text":true},{"remaining":7, "resetsAt":"2026-11-25T10:31:24+08:00"},{"remaining":80}]}
        ]})).unwrap();
        let card = build_card(usage, Ok(quotas), now);
        assert_eq!(card.value, "$158.1");
        assert_eq!(card.detail, "4 次请求 · 2 会话 · 1 错");
        let CardBlock::Metrics { items, .. } = &card.blocks[0] else {
            panic!()
        };
        assert_eq!(items[0].value, "2.0K");
        assert_eq!(items[1].value, "90%");
        let CardBlock::Meters { items, .. } = &card.blocks[1] else {
            panic!()
        };
        assert_eq!(
            items
                .iter()
                .map(|m| (m.label.as_str(), m.percent))
                .collect::<Vec<_>>(),
            [("opus-5-5", 50), ("two", 40), ("其他", 10)]
        );
        let CardBlock::Meters { items, .. } = &card.blocks[2] else {
            panic!()
        };
        assert_eq!(items[0].percent, 7);
        assert_eq!(items[0].detail, "17分重置");
        assert_eq!(
            items[1].detail,
            format!("缓存 {}", now.format("%m-%d %H:%M"))
        );
        assert!(items.iter().all(|m| m.tone == CardTone::Neutral));
    }

    #[test]
    #[ignore = "requires a running local Magpie gateway with subscription quotas"]
    fn local_gateway_produces_active_quota_rows() {
        let cards = read_cards();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].state, CardState::Ready);
        let Some(CardBlock::Meters { items, .. }) = cards[0].blocks.last() else {
            panic!("missing quota block")
        };
        let valid = items
            .iter()
            .filter(|row| row.value == "已用尽" || row.value.starts_with("剩 "))
            .count();
        assert!(valid > 0, "no numeric quota rows");
        println!("ready, {valid} numeric quota rows");
    }

    #[test]
    fn empty_usage_and_null_go_slice_show_empty_states() {
        let usage = serde_json::from_value(json!({"calls":0,"models":null})).unwrap();
        let quotas = serde_json::from_value(json!({"data":null})).unwrap();
        let card = build_card(usage, Ok(quotas), Local::now());
        assert_eq!(card.value, "今天还没有请求");
        let [CardBlock::Meters { items, .. }] = &card.blocks[..] else {
            panic!()
        };
        assert_eq!(items[0].value, "暂无配额");
        assert!(serde_json::from_value::<Usage>(json!({})).is_err());
        assert!(serde_json::from_value::<QuotaList>(json!({})).is_err());
    }
}
