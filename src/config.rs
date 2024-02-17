use easy_error::format_err;
use lazy_static::lazy_static;
use log::debug;
use outscale_api::apis::configuration::AWSv4Key;
use secrecy::Secret;
use secrecy::SecretString;
use serde::Deserialize;
use std::env;
use std::error::Error;
use std::fs::read_to_string;
use std::str::FromStr;
use std::sync::RwLock;

type CloudConfig = outscale_api::apis::configuration::Configuration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const METADATA_SUBREGION_URL: &str =
    "http://169.254.169.254/latest/meta-data/placement/availability-zone";
const METADATA_VMID_URL: &str = "http://169.254.169.254/latest/meta-data/instance-id";

lazy_static! {
    pub static ref CLOUD_CONFIG: RwLock<CloudConfig> = RwLock::new(CloudConfig::new());
    pub static ref REGION: RwLock<String> = RwLock::new(String::new());
    pub static ref SUBREGION: RwLock<String> = RwLock::new(String::new());
    pub static ref VM_ID: RwLock<String> = RwLock::new(String::new());
}
#[derive(Deserialize, Debug)]
pub struct Config {
    pub drives: Vec<ConfigFileDrive>,
}

pub fn discover_vm_config() -> Result<(), Box<dyn Error>> {
    debug!("getting subregion from metadata");
    let subregion = reqwest::blocking::get(METADATA_SUBREGION_URL)?.text()?;
    let mut region = subregion.clone();
    region.pop();
    {
        *SUBREGION.write()? = subregion;
        *REGION.write()? = region;
    }
    debug!("get vm id");
    let vm_id = reqwest::blocking::get(METADATA_VMID_URL)?.text()?;
    {
        *VM_ID.write()? = vm_id;
    }
    Ok(())
}

pub fn region() -> Result<String, Box<dyn Error>> {
    Ok(String::from(&(*REGION.read()?)))
}

pub fn load(path: String) -> Result<Config, Box<dyn Error>> {
    debug!("trying to read \"{}\"", path);
    let data = read_to_string(path)?;
    let config_file: ConfigFile = serde_json::from_str(&data)?;

    let config_file_auth = match config_file.authentication {
        Some(c) => c,
        None => {
            debug!("cannot get credentials through configuration file, trying to get credentials through env");
            let Ok(access_key) = env::var("OSC_ACCESS_KEY") else {
                return Err(Box::new(format_err!(
                    "Cannot get OSC_ACCESS_KEY env variable"
                )));
            };
            let Ok(secret_key) = env::var("OSC_SECRET_KEY") else {
                return Err(Box::new(format_err!(
                    "Cannot get OSC_SECRET_KEY env variable"
                )));
            };
            ConfigFileAuth {
                access_key,
                secret_key: SecretString::new(secret_key),
            }
        }
    };
    discover_vm_config()?;

    debug!("forge cloud configuration");
    let mut cloud_config = CloudConfig::new();
    cloud_config.aws_v4_key = Some(AWSv4Key {
        access_key: config_file_auth.access_key,
        secret_key: config_file_auth.secret_key,
        region: region()?,
        service: "oapi".to_string(),
    });
    cloud_config.user_agent = Some(format!("bsud/{}", VERSION));
    {
        *CLOUD_CONFIG.write()? = cloud_config;
    }

    Ok(Config {
        drives: config_file.drives,
    })
}

#[derive(Deserialize, Debug)]
struct ConfigFile {
    authentication: Option<ConfigFileAuth>,
    drives: Vec<ConfigFileDrive>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFileAuth {
    access_key: String,
    secret_key: Secret<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFileDrive {
    pub name: String,
    pub target: DriveTarget,
    pub mount_path: String,
    pub disk_type: Option<DiskType>,
    pub disk_iops_per_gib: Option<usize>,
    pub max_total_size_gib: Option<usize>,
    pub initial_size_gib: Option<usize>,
    pub max_bsu_count: Option<usize>,
    pub max_used_space_perc: Option<usize>,
    pub min_used_space_perc: Option<usize>,
    pub disk_scale_factor_perc: Option<usize>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum DriveTarget {
    Online,  // normal  drive flow, drive is available
    Offline, // unmount + detach from VM
    Delete,  // unmount + detach from VM + delete data
}

impl FromStr for DriveTarget {
    type Err = ();
    fn from_str(input: &str) -> Result<DriveTarget, Self::Err> {
        match input.to_lowercase().as_str() {
            "online" => Ok(DriveTarget::Online),
            "offline" => Ok(DriveTarget::Offline),
            "delete" => Ok(DriveTarget::Delete),
            _ => Err(()),
        }
    }
}

impl ToString for DriveTarget {
    fn to_string(&self) -> String {
        match self {
            DriveTarget::Online => String::from("online"),
            DriveTarget::Offline => String::from("offline"),
            DriveTarget::Delete => String::from("delete"),
        }
    }
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DiskType {
    Standard,
    Gp2,
    Io1,
}

impl FromStr for DiskType {
    type Err = ();
    fn from_str(input: &str) -> Result<DiskType, Self::Err> {
        match input.to_lowercase().as_str() {
            "standard" => Ok(Self::Standard),
            "gp2" => Ok(Self::Gp2),
            "io1" => Ok(Self::Io1),
            _ => Err(()),
        }
    }
}

impl ToString for DiskType {
    fn to_string(&self) -> String {
        match self {
            Self::Standard => "standard".to_string(),
            Self::Gp2 => "gp2".to_string(),
            Self::Io1 => "io1".to_string(),
        }
    }
}
