use std::{io, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use revault_net::noise::PublicKey as NoisePubkey;
use revault_tx::{
    bitcoin::{hashes::hex::FromHex, Network},
    scripts::{CpfpDescriptor, DepositDescriptor, UnvaultDescriptor},
};

use serde::{de, Deserialize, Deserializer};

fn deserialize_fromstr<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let string = String::deserialize(deserializer)?;
    T::from_str(&string)
        .map_err(|e| de::Error::custom(format!("Error parsing descriptor '{}': '{}'", string, e)))
}

fn deserialize_noisepubkey<'de, D>(deserializer: D) -> Result<NoisePubkey, D::Error>
where
    D: Deserializer<'de>,
{
    let data = String::deserialize(deserializer)?;
    FromHex::from_hex(&data)
        .map_err(de::Error::custom)
        .map(NoisePubkey)
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

fn deserialize_loglevel<'de, D>(deserializer: D) -> Result<log::LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    let level_str = String::deserialize(deserializer)?;
    log::LevelFilter::from_str(&level_str).map_err(de::Error::custom)
}

fn default_loglevel() -> log::LevelFilter {
    log::LevelFilter::Info
}

fn default_poll_interval() -> Duration {
    Duration::from_secs(10)
}

fn daemon_default() -> bool {
    false
}

/// Everything we need to know for talking to bitcoind serenely
#[derive(Debug, Clone, Deserialize)]
pub struct BitcoindConfig {
    /// The network we are operating on, one of "bitcoin", "testnet", "regtest"
    pub network: Network,
    /// Path to bitcoind's cookie file, to authenticate the RPC connection
    pub cookie_path: PathBuf,
    /// The IP:port bitcoind's RPC is listening on
    pub addr: SocketAddr,
    /// The poll interval for bitcoind
    #[serde(
        deserialize_with = "deserialize_duration",
        default = "default_poll_interval"
    )]
    pub poll_interval_secs: Duration,
}

#[derive(Debug, Deserialize)]
pub struct ScriptsConfig {
    #[serde(deserialize_with = "deserialize_fromstr")]
    pub deposit_descriptor: DepositDescriptor,
    #[serde(deserialize_with = "deserialize_fromstr")]
    pub unvault_descriptor: UnvaultDescriptor,
    #[serde(deserialize_with = "deserialize_fromstr")]
    pub cpfp_descriptor: CpfpDescriptor,
}

/// Static informations we require to operate
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Everything we need to know to talk to bitcoind
    pub bitcoind_config: BitcoindConfig,
    /// Bitcoin Miniscript descriptors to be able to derive the transaction chain
    pub scripts_config: ScriptsConfig,
    /// The Noise static public keys of "our" stakeholder
    #[serde(deserialize_with = "deserialize_noisepubkey")]
    pub stakeholder_noise_key: NoisePubkey,
    /// The host of the sync server (may be an IP or a hidden service)
    pub coordinator_host: String,
    /// The Noise static public key of the sync server
    #[serde(deserialize_with = "deserialize_noisepubkey")]
    pub coordinator_noise_key: NoisePubkey,
    /// An optional custom data directory
    // TODO: have a default implem as in cosignerd
    pub data_dir: Option<PathBuf>,
    /// Whether to daemonize the process
    #[serde(default = "daemon_default")]
    pub daemon: bool,
    /// What messages to log
    #[serde(
        deserialize_with = "deserialize_loglevel",
        default = "default_loglevel"
    )]
    pub log_level: log::LevelFilter,
    /// <ip:port> to bind to
    // TODO: have a default implem as in cosignerd
    pub listen: Option<SocketAddr>,
}

#[derive(Debug)]
pub enum ConfigError {
    NoConfigDir,
    ReadingConfigFile(io::Error),
    ParsingConfigFile(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ConfigError::NoConfigDir => write!(f, "Could not locate the configuration directory."),
            ConfigError::ReadingConfigFile(e) => write!(f, "Error reading config file: '{}'", e),
            ConfigError::ParsingConfigFile(e) => write!(f, "Error parsing config file: '{}'", e),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Get the absolute path to the revault configuration folder.
///
/// It's a "miradord/<network>/" directory in the XDG standard configuration directory for
/// all OSes but Linux-based ones, for which it's `~/.miradord/<network>/`.
/// There is only one config file at `revault/config.toml`, which specifies the network.
/// Rationale: we want to have the database, RPC socket, etc.. in the same folder as the
/// configuration file but for Linux the XDG specifoes a data directory (`~/.local/share/`)
/// different from the configuration one (`~/.config/`).
pub fn config_folder_path() -> Result<PathBuf, ConfigError> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".miradord");

        #[cfg(not(target_os = "linux"))]
        path.push("Miradord");

        return Ok(path);
    }

    Err(ConfigError::NoConfigDir)
}

fn config_file_path() -> Result<PathBuf, ConfigError> {
    config_folder_path().map(|mut path| {
        path.push("config.toml");
        path
    })
}

impl Config {
    /// Get our static configuration out of a mandatory configuration file.
    ///
    /// We require all settings to be set in the configuration file, and only in the configuration
    /// file. We don't allow to set them via the command line or environment variables to avoid a
    /// futile duplication.
    pub fn from_file(custom_path: Option<PathBuf>) -> Result<Config, ConfigError> {
        let config_file = custom_path.unwrap_or(config_file_path()?);

        let config = std::fs::read(&config_file)
            .map_err(ConfigError::ReadingConfigFile)
            .and_then(|file_content| {
                toml::from_slice::<Config>(&file_content).map_err(ConfigError::ParsingConfigFile)
            })?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::{config_file_path, Config};

    // Test the format of the configuration file
    #[test]
    fn deserialize_toml_config() {
        // A valid config
        let toml_str = r#"
            daemon = false
            log_level = "trace"
            data_dir = "/home/wizardsardine/custom/folder/"

            stakeholder_noise_key = "3de4539519b6baca35ad14cd5bac9a4e0875a851632112405bb0547e6fcf16f6"

            coordinator_host = "127.0.0.1:1"
            coordinator_noise_key = "d91563973102454a7830137e92d0548bc83b4ea2799f1df04622ca1307381402"

            [scripts_config]
            cpfp_descriptor = "wsh(thresh(1,pk(xpub6BaZSKgpaVvibu2k78QsqeDWXp92xLHZxiu1WoqLB9hKhsBf3miBUDX7PJLgSPvkj66ThVHTqdnbXpeu8crXFmDUd4HeM4s4miQS2xsv3Qb/*)))#cwycq5xu"
            deposit_descriptor = "wsh(multi(2,xpub6AHA9hZDN11k2ijHMeS5QqHx2KP9aMBRhTDqANMnwVtdyw2TDYRmF8PjpvwUFcL1Et8Hj59S3gTSMcUQ5gAqTz3Wd8EsMTmF3DChhqPQBnU/*,xpub6AaffFGfH6WXfm6pwWzmUMuECQnoLeB3agMKaLyEBZ5ZVfwtnS5VJKqXBt8o5ooCWVy2H87GsZshp7DeKE25eWLyd1Ccuh2ZubQUkgpiVux/*))#n3cj9mhy"
            unvault_descriptor = "wsh(andor(thresh(1,pk(xpub6BaZSKgpaVvibu2k78QsqeDWXp92xLHZxiu1WoqLB9hKhsBf3miBUDX7PJLgSPvkj66ThVHTqdnbXpeu8crXFmDUd4HeM4s4miQS2xsv3Qb/*)),and_v(v:multi(2,03b506a1dbe57b4bf48c95e0c7d417b87dd3b4349d290d2e7e9ba72c912652d80a,0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce),older(4)),thresh(2,pkh(xpub6AHA9hZDN11k2ijHMeS5QqHx2KP9aMBRhTDqANMnwVtdyw2TDYRmF8PjpvwUFcL1Et8Hj59S3gTSMcUQ5gAqTz3Wd8EsMTmF3DChhqPQBnU/*),a:pkh(xpub6AaffFGfH6WXfm6pwWzmUMuECQnoLeB3agMKaLyEBZ5ZVfwtnS5VJKqXBt8o5ooCWVy2H87GsZshp7DeKE25eWLyd1Ccuh2ZubQUkgpiVux/*))))#532k8uvf"

            [bitcoind_config]
            network = "bitcoin"
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            poll_interval_secs = 18
        "#;
        toml::from_str::<Config>(toml_str).expect("Deserializing valid toml_str");

        // Invalid descriptors checksum
        let toml_str = r#"
            daemon = false
            log_level = "trace"
            data_dir = "/home/wizardsardine/custom/folder/"

            stakeholder_noise_key = "3de4539519b6baca35ad14cd5bac9a4e0875a851632112405bb0547e6fcf16f6"

            coordinator_host = "127.0.0.1:1"
            coordinator_noise_key = "d91563973102454a7830137e92d0548bc83b4ea2799f1df04622ca1307381402"

            [scripts_config]
            cpfp_descriptor = "wsh(thresh(1,pk(xpub6BaZSKgpaVvibu2k78QsqeDWXp92xLHZxiu1WoqLB9hKhsBf3miBUDX7PJLgSPvkj66ThVHTqdnbXpeu8crXFmDUd4HeM4s4miQS2xsv3Qb/*)))#cwycq5xu"
            deposit_descriptor = "wsh(multi(2,xpub6AHA9hZDN11k2ijHMeS5QqHx2KP9aMBRhTDqANMnwVtdyw2TDYRmF8PjpvwUFcL1Et8Hj59S3gTSMcUQ5gAqTz3Wd8EsMTmF3DChhqPQBnU/*,xpub6AaffFGfH6WXfm6pwWzmUMuECQnoLeB3agMKaLyEBZ5ZVfwtnS5VJKqXBt8o5ooCWVy2H87GsZshp7DeKE25eWLyd1Ccuh2ZubQUkgpiVux/*))#n3cj9mhy"
            # The checksum is for older(4) but it was replaced by older(42)
            unvault_descriptor = "wsh(andor(thresh(1,pk(xpub6BaZSKgpaVvibu2k78QsqeDWXp92xLHZxiu1WoqLB9hKhsBf3miBUDX7PJLgSPvkj66ThVHTqdnbXpeu8crXFmDUd4HeM4s4miQS2xsv3Qb/*)),and_v(v:multi(2,03b506a1dbe57b4bf48c95e0c7d417b87dd3b4349d290d2e7e9ba72c912652d80a,0295e7f5d12a2061f1fd2286cefec592dff656a19f55f4f01305d6aa56630880ce),older(42)),thresh(2,pkh(xpub6AHA9hZDN11k2ijHMeS5QqHx2KP9aMBRhTDqANMnwVtdyw2TDYRmF8PjpvwUFcL1Et8Hj59S3gTSMcUQ5gAqTz3Wd8EsMTmF3DChhqPQBnU/*),a:pkh(xpub6AaffFGfH6WXfm6pwWzmUMuECQnoLeB3agMKaLyEBZ5ZVfwtnS5VJKqXBt8o5ooCWVy2H87GsZshp7DeKE25eWLyd1Ccuh2ZubQUkgpiVux/*))))#532k8uvf"

            [bitcoind_config]
            network = "bitcoin"
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            poll_interval_secs = 4
        "#;
        let config_res: Result<Config, toml::de::Error> = toml::from_str(toml_str);
        config_res.expect_err("Deserializing an invalid toml_str");

        // Not enough parameters
        let toml_str = r#"
            daemon = false
            log_level = "trace"
            data_dir = "/home/wizardsardine/custom/folder/"

            coordinator_host = "127.0.0.1:1"
            coordinator_noise_key = "d91563973102454a7830137e92d0548bc83b4ea2799f1df04622ca1307381402"
        "#;
        let config_res: Result<Config, toml::de::Error> = toml::from_str(toml_str);
        config_res.expect_err("Deserializing an invalid toml_str");
    }

    #[test]
    fn config_directory() {
        let filepath = config_file_path().expect("Getting config file path");

        #[cfg(target_os = "linux")]
        {
            assert!(filepath.as_path().starts_with("/home/"));
            assert!(filepath.as_path().ends_with(".miradord/config.toml"));
        }

        #[cfg(target_os = "macos")]
        assert!(filepath
            .as_path()
            .ends_with("Library/Application Support/Miradord/config.toml"));

        #[cfg(target_os = "windows")]
        assert!(filepath
            .as_path()
            .ends_with(r#"AppData\Roaming\Miradord\config.toml"#));
    }
}
