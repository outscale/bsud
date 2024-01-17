use bsudlib::config::{
    discover_vm_config, region, ConfigFileDrive, DiskType, DriveTarget, CLOUD_CONFIG,
};
use bsudlib::drive::{Drive, DriveCmd};
use bsudlib::{fs, lvm};
use bsudlib::utils::bytes_to_gib;
use cucumber::{given, then, when, writer, World, WriterExt};
use log::debug;
use std::error::Error;
use outscale_api::apis::configuration::AWSv4Key;
use secrecy::SecretString;
use std::cmp::Ordering;
use std::env;
use std::str::FromStr;
use std::sync::mpsc::{channel, Sender};
use std::fs::remove_file;
use std::time::Duration;
use tokio::time::sleep;
use rand::{distributions::Alphanumeric, Rng};
use std::fs::read_dir;
use std::path::PathBuf;
use std::io;
use tokio::task::block_in_place;
use async_process::Command;

fn setup_creds() {
    let mut global_cloud_config = CLOUD_CONFIG.write().expect("cloud config setting");
    if global_cloud_config.aws_v4_key.is_some() {
        debug!("credentials already set");
        return;
    }
    debug!("get credentials through env");
    let access_key = env::var("OSC_ACCESS_KEY").expect("OSC_ACCESS_KEY must be set");
    let secret_key =
        SecretString::new(env::var("OSC_SECRET_KEY").expect("OSC_SECRET_KEY must be set"));
    // This avoid async to crash with blocking request
     block_in_place(move || {
        discover_vm_config().expect("discover vm config");
    });
    global_cloud_config.aws_v4_key = Some(AWSv4Key {
        access_key,
        secret_key,
        region: region().expect("read region"),
        service: "oapi".to_string(),
    });
}

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct DriveEnv {
    drive: Drive,
    _cmd: Sender<DriveCmd>,
}

impl DriveEnv {
    fn new() -> Self {
        setup_creds();
        let (sender, receiver) = channel::<DriveCmd>();
        Self {
            drive: Drive::new(DriveEnv::drive_config(), receiver),
            _cmd: sender,
        }
    }

    fn drive_config() -> ConfigFileDrive {
        let random_name = random_name();
        ConfigFileDrive {
            name: format!("test-{}", random_name),
            target: DriveTarget::Online,
            mount_path: format!("/media/bsud-{}/", random_name),
            disk_type: Some(DiskType::Gp2),
            disk_iops_per_gib: None,
            max_bsu_count: Some(10),
            max_total_size_gib: None,
            initial_size_gib: Some(10),
            max_used_space_perc: Some(85),
            min_used_space_perc: Some(20),
            disk_scale_factor_perc: Some(20),
        }
    }
}

#[given(expr = "drive target is {word}")]
async fn drive_config_target(drive_env: &mut DriveEnv, target: String) {
    drive_env.drive.target = DriveTarget::from_str(&target).expect("drive target");
}

#[given(expr = "drive disk type is {word}")]
async fn drive_config_disk_type(drive_env: &mut DriveEnv, disk_type: String) {
    drive_env.drive.disk_type = DiskType::from_str(&disk_type).expect("disk type");
}

#[given(expr = "drive max bsu count is {int}")]
async fn drive_config_max_bsu_count(drive_env: &mut DriveEnv, count: usize) {
    drive_env.drive.max_bsu_count = count;
}

#[given(expr = "drive max total size is unlimited")]
async fn drive_config_max_total_size_unlimited(drive_env: &mut DriveEnv) {
    drive_env.drive.max_total_size_gib = None;
}

#[given(expr = "drive max total size is {int}Gib")]
async fn drive_config_max_total_size_gib(drive_env: &mut DriveEnv, max_gib: usize) {
    drive_env.drive.max_total_size_gib = Some(max_gib);
}

#[given(expr = "drive initial size is {int}Gib")]
async fn drive_config_initial_size_gib(drive_env: &mut DriveEnv, size_gib: usize) {
    drive_env.drive.initial_size_gib = size_gib;
}

#[given(expr = "drive max used space is {int}%")]
async fn drive_config_max_used_space_perc(drive_env: &mut DriveEnv, max_per: usize) {
    drive_env.drive.max_used_space_perc = max_per as f32 / 100.0;
}

#[given(expr = "drive min used space is {int}%")]
async fn drive_config_min_used_space_perc(drive_env: &mut DriveEnv, min_per: usize) {
    drive_env.drive.min_used_space_perc = min_per as f32 / 100.0;
}

#[given(expr = "drive scale factor is {int}%")]
async fn drive_config_disk_scale_factor_perc(drive_env: &mut DriveEnv, scale_per: usize) {
    drive_env.drive.disk_scale_factor_perc = scale_per as f32 / 100.0;
}

#[given(expr = "reconcile runs")]
#[when(expr = "reconcile runs")]
fn feed_cat(drive_env: &mut DriveEnv) {
    drive_env.drive.reconcile().expect("reconcile should not fail");
}

#[given(expr = "drive has no BSU")]
#[then(expr = "cleanup")]
async fn drive_has_no_bsu(drive_env: &mut DriveEnv) {
    drive_env
        .drive
        .reconcile_delete()
        .expect("reconcile deletion should be ok");
    drive_env
        .drive
        .fetch_all_drive_bsu()
        .expect("should be able to fetch drives");
    assert_eq!(drive_env.drive.bsu_count(), 0);
}

#[given(expr = "drive target is set to {word}")]
async fn drive_target_is_set_to(drive_env: &mut DriveEnv, target: String) {
    let target = DriveTarget::from_str(target.as_str()).expect("bad target for drive");
    drive_env.drive.target = target;
}

#[given(expr = "drive usage is {int}Gib")]
async fn drive_set_usage(drive_env: &mut DriveEnv, target_gib: usize) -> Result<(), Box<dyn Error>> {
    let lv_path = lvm::lv_path(&drive_env.drive.name);
    loop {
        wait_for_stabilized_usage(&drive_env.drive).await;
        let current_drive_usage_bytes = fs::used_bytes(&lv_path).expect("get drive usage");
        let current_drive_usage_gib = bytes_to_gib(current_drive_usage_bytes).round() as usize;
        match current_drive_usage_gib.cmp(&target_gib) {
            Ordering::Equal => {
                debug!("current_drive_usage_gib ({}) = target_gib ({})", current_drive_usage_gib, target_gib);
                return Ok(())
            },
            Ordering::Less => {
                debug!("current_drive_usage_gib ({}) < target_gib ({})", current_drive_usage_gib, target_gib);
                debug!("creating 1gib file");
                create_1_gib_file(&drive_env.drive.mount_path).await.expect("create file");
            },
            Ordering::Greater => {
                debug!("current_drive_usage_gib ({}) > target_gib ({})", current_drive_usage_gib, target_gib);
                debug!("removing 1gib file",);
                delete_1_gib_file(&drive_env.drive.mount_path).expect("remove file");
            },
        };
    }
}

fn random_name() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

async fn create_1_gib_file(folder: &str) -> Result<(), Box<dyn Error>> {
    let output_file = format!("of={}/{}.zero", folder, random_name());
    debug!("writing 1gib file to {}", output_file);
    let count = format!("count={}", 1024_usize.pow(2));
    let out = Command::new("dd")
        .args(["if=/dev/zero", output_file.as_str(), "bs=1024", count.as_str(), "conv=fsync"])
        .output().await?;
    assert!(out.status.success());
    Ok(())
}

fn delete_1_gib_file(folder: &str) -> Result<(), Box<dyn Error>> {
    let read = read_dir(PathBuf::from(folder))?;
    for entry in read {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            debug!("removing file {}", &entry.path().as_os_str().to_str().unwrap());
            remove_file(&entry.path())?;
        }
    }
    Ok(())
}

#[given(expr = "drive has {int} BSU")]
#[then(expr = "drive has {int} BSU")]
async fn drive_has_x_bsu(drive_env: &mut DriveEnv, bsu_count: usize) {
    drive_env
        .drive
        .fetch_all_drive_bsu()
        .expect("fetch all BSU from drive");
    assert_eq!(drive_env.drive.bsu_count(), bsu_count);
}

#[given(expr = "drive is mounted")]
#[then(expr = "drive is mounted")]
async fn drive_is_mounted(drive_env: &mut DriveEnv) {
    let lv_path = lvm::lv_path(&drive_env.drive.name);
    assert!(fs::is_mounted(&lv_path, &drive_env.drive.mount_path).expect("fs::is_mounted"))
}

#[given(expr = "drive size is {int}Gib")]
#[then(expr = "drive size is {int}Gib")]
async fn drive_has_x_gib(drive_env: &mut DriveEnv, supposed_capa_gib: usize) {
    let lv_path = lvm::lv_path(&drive_env.drive.name);
    let fs_size_bytes = fs::size_bytes(&lv_path).expect("get fs size");
    let fs_size_gib = bytes_to_gib(fs_size_bytes).round() as usize;
    assert_eq!(fs_size_gib, supposed_capa_gib);
}

async fn wait_for_stabilized_usage(drive: &Drive) {
    let lv_path = lvm::lv_path(&drive.name);
    let mut usage = fs::used_bytes(&lv_path).expect("get fs usage");
    loop {
        debug!("wait for file usage to stabilize");
        sleep(Duration::from_millis(10_000)).await;
        let new_usage = fs::used_bytes(&lv_path).expect("get fs usage");
        if usage == new_usage {
            return;
        }
        usage = new_usage;
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();
    DriveEnv::cucumber()
        .with_writer(
            writer::Basic::raw(io::stdout(), writer::Coloring::Never, 0)
                .summarized()
                .assert_normalized(),
            )
        .run("tests/features/")
        .await;
}
