//! A publisher uses the installed executable and its exit status, including with no UI running.
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn run(profile: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskisle"))
        .arg("plugins")
        .args(args)
        .env("APPDATA", profile)
        .env("DESKISLE_MARKET_INDEX", profile.join("market/index.json"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn market_list_and_install_share_the_hosts_plugin_directory() {
    use sha2::{Digest, Sha256};
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let profile = std::env::temp_dir().join(format!(
        "deskisle-market-cli-{}-{nonce}",
        std::process::id()
    ));
    let market = profile.join("market");
    fs::create_dir_all(market.join("packages")).unwrap();
    let package = serde_json::to_vec(&json!({
        "protocol":1, "name":"快捷键", "version":"1.0.0", "contributions":[{
            "point":"desktop.cards","version":1,"data":{"cards":[{"title":"显示桌面","value":"Win + D"}]}
        }]
    })).unwrap();
    let entry =
        json!({"id":"shortcuts","name":"快捷键","version":"1.0.0","description":"常用快捷键"});
    let mut indexed = entry.clone();
    indexed["sha256"] = json!(format!("{:x}", Sha256::digest(&package)));
    let source = market.join("packages/shortcuts.json");
    fs::write(&source, &package).unwrap();
    fs::write(
        market.join("index.json"),
        json!({"schema":1,"plugins":[indexed]}).to_string(),
    )
    .unwrap();

    let listed = run(&profile, &["list"], &[]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&listed.stdout).unwrap(),
        json!([entry])
    );
    assert!(!profile.join("DeskIsle").exists());

    let target = profile.join("DeskIsle/plugins/shortcuts.json");
    let install = run(&profile, &["install", "shortcuts"], &[]);
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert_eq!(fs::read(&target).unwrap(), package);
    fs::write(&source, b"broken download").unwrap();
    let failure = run(&profile, &["install", "shortcuts"], &[]);
    assert_eq!(failure.status.code(), Some(1));
    assert!(!failure.stderr.is_empty());
    assert_eq!(fs::read(&target).unwrap(), package);
    assert!(
        run(&profile, &["remove", "shortcuts"], &[])
            .status
            .success()
    );
    assert!(!target.exists());
    fs::remove_dir_all(profile).unwrap();
}

#[test]
fn publishers_can_discover_validate_replace_and_withdraw() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let profile =
        std::env::temp_dir().join(format!("deskisle-publisher-{}-{nonce}", std::process::id()));
    fs::create_dir(&profile).unwrap();

    let capabilities = run(&profile, &["capabilities"], &[]);
    assert!(capabilities.status.success());
    let capabilities: Value = serde_json::from_slice(&capabilities.stdout).unwrap();
    assert_eq!(capabilities["protocol"], 1);
    assert!(
        capabilities["points"]["desktop.cards"]
            .as_array()
            .unwrap()
            .contains(&json!(1))
    );

    // Exercise the exact document offered to authors in the protocol reference.
    let example = include_str!("../docs/plugins.md")
        .split("```json")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap()
        .trim()
        .as_bytes();
    let checked = run(&profile, &["validate", "test-provider"], example);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(checked["version"], "1.0.0");
    assert!(!profile.join("DeskIsle").exists());

    let source = profile.join("发布 数据.json");
    fs::write(&source, example).unwrap();
    let target = profile.join("DeskIsle/plugins/test-provider.json");
    assert!(
        run(
            &profile,
            &["publish", "test-provider", source.to_str().unwrap()],
            &[]
        )
        .status
        .success()
    );
    assert_eq!(fs::read(&target).unwrap(), example);

    let mut update: Value = serde_json::from_slice(example).unwrap();
    update["contributions"][0]["point"] = json!("vendor.future");
    let rejected = run(
        &profile,
        &["publish", "test-provider"],
        &serde_json::to_vec(&update).unwrap(),
    );
    assert_eq!(rejected.status.code(), Some(1));
    assert!(!rejected.stderr.is_empty());
    assert_eq!(fs::read(&target).unwrap(), example);

    update["contributions"][0]["required"] = json!(false);
    let optional = serde_json::to_vec(&update).unwrap();
    assert!(
        run(&profile, &["publish", "test-provider"], &optional)
            .status
            .success()
    );
    assert_eq!(fs::read(&target).unwrap(), optional);
    for _ in 0..2 {
        assert!(
            run(&profile, &["remove", "test-provider"], &[])
                .status
                .success()
        );
        assert!(!target.exists());
    }
    fs::remove_file(source).unwrap();
    fs::remove_dir(profile.join("DeskIsle/plugins")).unwrap();
    fs::remove_dir(profile.join("DeskIsle")).unwrap();
    fs::remove_dir(profile).unwrap();
}
