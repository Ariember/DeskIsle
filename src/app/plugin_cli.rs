//! Plugin management without starting the desktop host.

use crate::integrations::{market::Market, plugins};
use crate::{plugins::MAX_DOCUMENT_BYTES, storage};
use std::ffi::OsString;
use std::io::{self, Read, Write};

const USAGE: &str = "deskisle.exe plugins list\ndeskisle.exe plugins install <id>\ndeskisle.exe plugins capabilities\ndeskisle.exe plugins validate <id> [JSON文件]\ndeskisle.exe plugins publish <id> [JSON文件]\ndeskisle.exe plugins remove <id>";

pub(crate) fn run(args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let args: Vec<_> = args.collect();
    if matches!(args.as_slice(), [help] if help == "--help") {
        return writeln!(io::stdout().lock(), "{USAGE}");
    }
    if matches!(args.as_slice(), [capabilities] if capabilities == "capabilities") {
        return writeln!(io::stdout().lock(), "{}", crate::plugins::capabilities());
    }
    if matches!(args.as_slice(), [list] if list == "list") {
        let entries: Vec<_> = Market::load()?
            .entries
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "id": entry.id, "name": entry.name, "version": entry.version,
                    "description": entry.description
                })
            })
            .collect();
        return writeln!(io::stdout().lock(), "{}", serde_json::Value::Array(entries));
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, USAGE);
    let (verb, rest) = args.split_first().ok_or_else(invalid)?;
    let (id, rest) = rest.split_first().ok_or_else(invalid)?;
    let id = id.to_str().ok_or_else(invalid)?;
    let directory = || storage::plugins_directory().map_err(io::Error::other);
    match (verb.to_str(), rest) {
        (Some("install"), []) => Market::load()?.install(id, &directory()?),
        (Some("publish" | "validate"), [] | [_]) => {
            let input: Box<dyn Read> = match rest.first() {
                Some(path) => Box::new(std::fs::File::open(path)?),
                None => Box::new(io::stdin().lock()),
            };
            let mut bytes = Vec::new();
            input
                .take(MAX_DOCUMENT_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if verb == "publish" {
                plugins::publish(&directory()?, id, &bytes)
            } else {
                let publication = plugins::validate(id, &bytes)?;
                let result = serde_json::json!({
                    "name": publication.name,
                    "version": publication.version.to_string()
                });
                writeln!(io::stdout().lock(), "{result}")
            }
        }
        (Some("remove"), []) => plugins::remove(&directory()?, id),
        _ => Err(invalid()),
    }
}
