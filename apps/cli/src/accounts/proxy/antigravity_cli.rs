//! Explicit agy-pool profiles use their owning CLI's quota API and refresh flow.
//! A profile ID never selects an arbitrary executable or a different account.
use crate::{
    domain::*,
    error::ProviderError,
    providers::{ProviderContext, antigravity, process},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, BufReader};

pub(super) fn valid_profile_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn profile_home(home: &Path, id: &str) -> Result<PathBuf, ProviderError> {
    if !valid_profile_id(id) {
        return Err(ProviderError::InvalidData);
    }
    let root = home.join(".agy-profiles");
    let path = root.join(id);
    for directory in [&root, &path] {
        let metadata =
            std::fs::symlink_metadata(directory).map_err(|_| ProviderError::Authentication)?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(ProviderError::Authentication);
        }
    }
    let value: Value = serde_json::from_slice(&super::read_file(&root.join("accounts.json"))?)
        .map_err(|_| ProviderError::InvalidData)?;
    if !value.as_array().is_some_and(|accounts| {
        accounts.iter().any(|account| {
            account.get("id").and_then(Value::as_str) == Some(id)
                && account.get("home").and_then(Value::as_str).map(Path::new)
                    == Some(path.as_path())
        })
    }) {
        return Err(ProviderError::Authentication);
    }
    Ok(path)
}

fn credential_email(bytes: &[u8]) -> Result<String, ProviderError> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|_| ProviderError::Authentication)?
        .trim();
    let decoded;
    let bytes = if let Some(encoded) = raw.strip_prefix("go-keyring-base64:") {
        decoded = STANDARD
            .decode(encoded)
            .map_err(|_| ProviderError::Authentication)?;
        decoded.as_slice()
    } else {
        raw.as_bytes()
    };
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ProviderError::Authentication)?;
    value
        .get("id_token_email")
        .and_then(Value::as_str)
        .filter(|email| {
            !email.is_empty() && email.len() <= 512 && !email.chars().any(char::is_control)
        })
        .map(str::to_owned)
        .ok_or(ProviderError::Authentication)
}

async fn identity(home: &Path) -> Result<String, ProviderError> {
    let keychain = home.join("Library/Keychains/login.keychain-db");
    let metadata =
        std::fs::symlink_metadata(&keychain).map_err(|_| ProviderError::Authentication)?;
    if !metadata.is_file() || metadata.is_symlink() {
        return Err(ProviderError::Authentication);
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        process::output(
            Path::new("/usr/bin/security"),
            &[
                "find-generic-password",
                "-s",
                "gemini",
                "-a",
                "antigravity",
                "-w",
                keychain.to_str().ok_or(ProviderError::InvalidData)?,
            ],
        ),
    )
    .await
    .map_err(|_| ProviderError::Timeout)??;
    credential_email(&output)
}

fn usage(bytes: &[u8], email: &str, now: OffsetDateTime) -> Result<ProviderUsage, ProviderError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidData)?;
    if value.get("status").and_then(Value::as_str) != Some("SUCCESS")
        || value.pointer("/command/name").and_then(Value::as_str) != Some("usage")
    {
        return Err(ProviderError::QuotaUnavailable);
    }
    let mut windows = antigravity::summary(
        value
            .pointer("/command/data")
            .ok_or(ProviderError::InvalidData)?,
        now,
    )?;
    for window in &mut windows {
        window.provenance.source = "antigravity_cli_usage".into();
    }
    Ok(ProviderUsage {
        fresh_observation: false,
        history_identity: None,
        reset_credits: None,
        antigravity_subscription: None,
        codex_profile: None,
        codex_reset_credits: None,
        diagnostics: vec![],
        account_ref: None,
        provider: ProviderId("antigravity".into()),
        account: AccountIdentity {
            verified: None,
            subscription_status: None,
            plan: None,
            id: email.into(),
            label: email.into(),
        },
        windows,
    })
}

pub(super) async fn fetch(
    context: &ProviderContext,
    profile: &str,
    email: &str,
) -> Result<ProviderUsage, ProviderError> {
    let home = directories::BaseDirs::new().ok_or(ProviderError::Authentication)?;
    let home = profile_home(home.home_dir(), profile)?;
    if identity(&home).await? != email {
        return Err(ProviderError::Authentication);
    }
    let program = process::resolve_program(Path::new("agy")).ok_or(ProviderError::CliNotFound)?;
    let mut child = tokio::process::Command::new(program)
        .args(["-p", "/usage", "--output-format", "json"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("NO_COLOR", "1")
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ProviderError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(ProviderError::Internal)?;
    let mut bytes = Vec::new();
    BufReader::new(stdout)
        .take((process::MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ProviderError::InvalidData)?;
    if bytes.len() > process::MAX_BYTES {
        return Err(ProviderError::InvalidData);
    }
    if !child
        .wait()
        .await
        .map_err(|_| ProviderError::Unavailable)?
        .success()
    {
        return Err(ProviderError::QuotaUnavailable);
    }
    if identity(&home).await? != email {
        return Err(ProviderError::Authentication);
    }
    usage(&bytes, email, context.clock.now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_selection_rejects_paths_and_requires_registered_home() {
        for id in ["", "../work-1", "/tmp/work-1", "work_1", "work-1;echo"] {
            assert!(!valid_profile_id(id));
        }
        assert!(valid_profile_id("work-1"));
        let root = std::env::temp_dir().join(format!(
            "quotio-agy-{}",
            crate::accounts::random_string().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let pool = root.join(".agy-profiles");
        let home = pool.join("work-1");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            pool.join("accounts.json"),
            json!([{"id":"work-1","home":home}]).to_string(),
        )
        .unwrap();
        assert_eq!(profile_home(&root, "work-1").unwrap(), home);
        std::fs::write(
            pool.join("accounts.json"),
            json!([{"id":"work-1","home":"/tmp/other"}]).to_string(),
        )
        .unwrap();
        assert!(profile_home(&root, "work-1").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_keychain_identity_without_exporting_tokens() {
        let raw = json!({"id_token_email":"work@example.com","token":{"access_token":"fixture"}})
            .to_string();
        let encoded = format!("go-keyring-base64:{}", STANDARD.encode(raw));
        assert_eq!(
            credential_email(encoded.as_bytes()).unwrap(),
            "work@example.com"
        );
        assert!(credential_email(br#"{"token":{"access_token":"fixture"}}"#).is_err());
    }

    #[test]
    fn cli_usage_preserves_windows_and_rejects_invalid_or_failed_results() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let mut value = json!({"status":"SUCCESS","command":{"name":"usage","data":{"groups":[
            {"name":"Gemini Models","buckets":[{"id":"gemini-weekly","remaining_fraction":0.49,"reset_time":"2026-11-17T01:02:39Z"}]},
            {"name":"Claude and GPT models","buckets":[{"id":"3p-weekly","remaining_fraction":1.0}]}
        ]}}});
        let result = usage(value.to_string().as_bytes(), "work@example.com", now).unwrap();
        assert_eq!(result.windows.len(), 2);
        assert_eq!(result.account.label, "work@example.com");
        assert!(result.windows[0].resets_at.is_some());
        assert_eq!(result.windows[0].provenance.source, "antigravity_cli_usage");
        value["command"]["data"]["groups"][0]["buckets"][0]["remaining_fraction"] = json!(1.1);
        assert!(usage(value.to_string().as_bytes(), "work@example.com", now).is_err());
        value["status"] = json!("ERROR");
        assert!(usage(value.to_string().as_bytes(), "work@example.com", now).is_err());
    }
}
