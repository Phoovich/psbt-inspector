use std::path::PathBuf;
use thiserror::Error;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Cannot read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Cannot parse config TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

// ── Config struct ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Config {
    pub api_key: String,
    pub network: String,
    pub ai_model: String,
    /// Opt-out: set to `false` to never send the PSBT/multisig context to
    /// the AI assistant. Even when `true`, the user is asked for
    /// session-level consent before the first request (S6).
    pub ai_send_context: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api_key: String::new(),
            network: "testnet".into(),
            ai_model: "claude-sonnet-4-5".into(),
            ai_send_context: true,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load config from `~/.config/psbt-inspector/config.toml`.
/// Missing file → `Config::default()`. Env vars applied last and always win.
///
/// Returns app-level startup warnings (S5 loose file permissions, S7 an
/// unrecognised `network` value) for display in the title bar — distinct
/// from the per-PSBT warnings in `PsbtSummary.warnings`.
pub fn load_config() -> Result<(Config, Vec<String>), ConfigError> {
    let (mut config, mut warnings) = load_from_path(config_path())?;
    apply_env_overrides(&mut config);

    if let Some(warning) = validate_network(&mut config) {
        warnings.push(warning);
    }

    Ok((config, warnings))
}

/// Validates `config.network`, resetting it to `"testnet"` and returning a
/// startup warning if it is not a recognised network (S7) — never silently
/// falls back to an unexpected network.
fn validate_network(config: &mut Config) -> Option<String> {
    if parse_network(&config.network).is_some() {
        return None;
    }
    let warning = format!(
        "config: network \"{}\" is not recognised — using testnet",
        config.network
    );
    config.network = "testnet".into();
    Some(warning)
}

/// Returns `~/.config/psbt-inspector/config.toml`.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("psbt-inspector")
        .join("config.toml")
}

/// Map a network string to a `bitcoin::Network`.
/// Accepts `"bitcoin"` / `"mainnet"`, `"testnet"`, `"signet"`, `"regtest"`.
/// Returns `None` for unrecognised values.
pub fn parse_network(s: &str) -> Option<bitcoin::Network> {
    match s.to_lowercase().as_str() {
        "bitcoin" | "mainnet" => Some(bitcoin::Network::Bitcoin),
        "testnet" => Some(bitcoin::Network::Testnet),
        "signet" => Some(bitcoin::Network::Signet),
        "regtest" => Some(bitcoin::Network::Regtest),
        _ => None,
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn load_from_path(path: PathBuf) -> Result<(Config, Vec<String>), ConfigError> {
    if !path.exists() {
        return Ok((Config::default(), Vec::new()));
    }
    let mut warnings = Vec::new();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                warnings.push(format!(
                    "config file {} is readable by other users (mode {:o}) — \
                     it may contain your Anthropic API key in plaintext; \
                     consider `chmod 600` or using PSBT_INSPECTOR_API_KEY instead",
                    path.display(),
                    mode & 0o777
                ));
            }
        }
    }

    let content = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok((config, warnings))
}

fn apply_env_overrides(config: &mut Config) {
    if let Ok(key) = std::env::var("PSBT_INSPECTOR_API_KEY") {
        config.api_key = key;
    }
    if let Ok(net) = std::env::var("PSBT_INSPECTOR_NETWORK") {
        config.network = net;
    }
    if let Ok(model) = std::env::var("PSBT_INSPECTOR_AI_MODEL") {
        config.ai_model = model;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialise tests that mutate env vars — cargo runs tests in parallel by default.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ─── Default ─────────────────────────────────────────────────────────────

    #[test]
    fn default_api_key_is_empty() {
        assert_eq!(Config::default().api_key, "");
    }

    #[test]
    fn default_network_is_testnet() {
        assert_eq!(Config::default().network, "testnet");
    }

    #[test]
    fn default_model_is_claude_sonnet() {
        assert_eq!(Config::default().ai_model, "claude-sonnet-4-5");
    }

    // ─── File loading ─────────────────────────────────────────────────────────

    #[test]
    fn missing_file_returns_default() {
        let (config, _) =
            load_from_path(PathBuf::from("/nonexistent/psbt_test/config.toml")).unwrap();
        assert_eq!(config.network, "testnet");
    }

    #[test]
    fn loads_all_fields_from_toml_file() {
        let dir = std::env::temp_dir().join("psbt_inspector_cfg_full");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "api_key = \"file-key\"\nnetwork = \"mainnet\"\nai_model = \"claude-opus-4-8\"\n",
        )
        .unwrap();
        let (config, _) = load_from_path(path).unwrap();
        assert_eq!(config.api_key, "file-key");
        assert_eq!(config.network, "mainnet");
        assert_eq!(config.ai_model, "claude-opus-4-8");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_fields_in_toml_use_defaults() {
        let dir = std::env::temp_dir().join("psbt_inspector_cfg_partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "api_key = \"only-key\"\n").unwrap();
        let (config, _) = load_from_path(path).unwrap();
        assert_eq!(config.api_key, "only-key");
        assert_eq!(config.network, "testnet");
        assert_eq!(config.ai_model, "claude-sonnet-4-5");
        let _ = std::fs::remove_dir_all(dir);
    }

    // ─── Env var overrides ───────────────────────────────────────────────────

    #[test]
    fn env_var_overrides_api_key() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: serialised by ENV_LOCK; no other test sets this var concurrently.
        unsafe {
            std::env::set_var("PSBT_INSPECTOR_API_KEY", "env-api-key");
        }
        let mut config = Config::default();
        apply_env_overrides(&mut config);
        unsafe {
            std::env::remove_var("PSBT_INSPECTOR_API_KEY");
        }
        assert_eq!(config.api_key, "env-api-key");
    }

    #[test]
    fn env_var_overrides_network() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("PSBT_INSPECTOR_NETWORK", "mainnet");
        }
        let mut config = Config::default();
        apply_env_overrides(&mut config);
        unsafe {
            std::env::remove_var("PSBT_INSPECTOR_NETWORK");
        }
        assert_eq!(config.network, "mainnet");
    }

    #[test]
    fn env_var_overrides_ai_model() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("PSBT_INSPECTOR_AI_MODEL", "claude-opus-4-8");
        }
        let mut config = Config::default();
        apply_env_overrides(&mut config);
        unsafe {
            std::env::remove_var("PSBT_INSPECTOR_AI_MODEL");
        }
        assert_eq!(config.ai_model, "claude-opus-4-8");
    }

    // ─── parse_network ────────────────────────────────────────────────────────

    #[test]
    fn parse_network_accepts_bitcoin() {
        assert!(matches!(
            parse_network("bitcoin"),
            Some(bitcoin::Network::Bitcoin)
        ));
    }

    #[test]
    fn parse_network_accepts_mainnet_alias() {
        assert!(matches!(
            parse_network("mainnet"),
            Some(bitcoin::Network::Bitcoin)
        ));
    }

    #[test]
    fn parse_network_accepts_testnet() {
        assert!(matches!(
            parse_network("testnet"),
            Some(bitcoin::Network::Testnet)
        ));
    }

    #[test]
    fn parse_network_accepts_signet() {
        assert!(matches!(
            parse_network("signet"),
            Some(bitcoin::Network::Signet)
        ));
    }

    #[test]
    fn parse_network_accepts_regtest() {
        assert!(matches!(
            parse_network("regtest"),
            Some(bitcoin::Network::Regtest)
        ));
    }

    #[test]
    fn parse_network_rejects_invalid_string() {
        assert!(parse_network("ethereumnet").is_none());
    }

    #[test]
    fn parse_network_rejects_empty_string() {
        assert!(parse_network("").is_none());
    }

    // ─── validate_network (S7) ───────────────────────────────────────────────

    #[test]
    fn validate_network_warns_and_resets_on_invalid_value() {
        let mut config = Config {
            network: "minnet".into(),
            ..Config::default()
        };
        let warning = validate_network(&mut config).expect("should warn");
        assert!(warning.contains("minnet"), "warning: {warning}");
        assert_eq!(config.network, "testnet");
    }

    #[test]
    fn validate_network_is_silent_on_valid_value() {
        let mut config = Config {
            network: "signet".into(),
            ..Config::default()
        };
        assert!(validate_network(&mut config).is_none());
        assert_eq!(config.network, "signet");
    }

    // ─── S5: config file permission warning ─────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn loose_permissions_produce_a_warning() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("psbt_inspector_cfg_perm_loose");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "api_key = \"x\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let (_, warnings) = load_from_path(path).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("readable by other")),
            "warnings: {warnings:?}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn strict_permissions_produce_no_warning() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("psbt_inspector_cfg_perm_strict");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "api_key = \"x\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let (_, warnings) = load_from_path(path).unwrap();
        assert!(warnings.is_empty(), "warnings: {warnings:?}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parse_network_is_case_insensitive() {
        assert!(matches!(
            parse_network("Testnet"),
            Some(bitcoin::Network::Testnet)
        ));
        assert!(matches!(
            parse_network("MAINNET"),
            Some(bitcoin::Network::Bitcoin)
        ));
        assert!(matches!(
            parse_network("Regtest"),
            Some(bitcoin::Network::Regtest)
        ));
    }
}
