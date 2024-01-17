use crate::utils::bytes_to_gib;
use crate::utils::exec;
use easy_error::format_err;
use lfs_core::{self, Stats};
use log::debug;
use proc_mounts::MountList;
use std::error::Error;
use std::fs::create_dir;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

pub fn device_seems_formated(device_path: &String) -> Result<bool, Box<dyn Error>> {
    debug!("does device {} seems formated ?", device_path);
    // Read fs header, consider unformated if reading only zeros
    let mut buffer = [0; 1_000_000];
    let mut file = File::open(device_path)?;
    let n = file.read(&mut buffer[..])?;
    for byte in &buffer[..n] {
        if *byte != 0 {
            debug!("does device {} seems formated ? -> true", device_path);
            return Ok(true);
        }
    }
    debug!("does device {} seems formated ? -> false", device_path);
    Ok(false)
}

pub fn format(device_path: &String) -> Result<(), Box<dyn Error>> {
    exec("mkfs.btrfs", &[device_path])?;
    Ok(())
}

pub fn is_folder(path: &String) -> bool {
    PathBuf::from(path).is_dir()
}

pub fn create_folder(path: &String) -> Result<(), Box<dyn Error>> {
    Ok(create_dir(path)?)
}

pub fn is_mounted(device_path: &String, mount_target: &String) -> Result<bool, Box<dyn Error>> {
    let mount_list = MountList::new()?;
    let source = Path::new(device_path.as_str());
    let Some(mount_info) = mount_list.get_mount_by_source(source) else {
        debug!("{} is not mounted", device_path);
        return Ok(false);
    };
    let dest = PathBuf::from(mount_target.clone());
    if mount_info.dest != dest {
        return Err(Box::new(format_err!(
            "{:?} seems to be mounted on {:?}, not in {}",
            source,
            mount_info.dest,
            mount_target
        )));
    }
    debug!(
        "{:?} is mounted on {:?}, all good",
        mount_info.source, mount_info.dest
    );
    Ok(true)
}

pub fn mount(device_path: &String, mount_target: &String) -> Result<(), Box<dyn Error>> {
    exec("mount", &[device_path, mount_target])?;
    Ok(())
}

pub fn umount(device_path: &String) -> Result<(), Box<dyn Error>> {
    exec("umount", &[device_path])?;
    Ok(())
}

fn get_stats(device_path: &String) -> Result<Option<Stats>, Box<dyn Error>> {
    let mut read_options = lfs_core::ReadOptions::default();
    read_options.remote_stats(false);
    for mount in lfs_core::read_mounts(&read_options)? {
        if mount.info.fs == *device_path {
            let stats = mount.stats?;
            return Ok(Some(stats));
        }
    }
    Ok(None)
}

pub fn used_bytes(device_path: &String) -> Result<usize, Box<dyn Error>> {
    debug!("used_bytes");
    let Some(stats) = get_stats(device_path)? else {
        return Err(Box::new(format_err!(
            "used_bytes cannot get fs stats from {}",
            device_path
        )));
    };
    let used_bytes = stats.used() as usize;
    debug!(
        "used_bytes on {}: {}B ({}GiB)",
        device_path,
        used_bytes,
        bytes_to_gib(used_bytes)
    );
    Ok(used_bytes)
}

pub fn size_bytes(device_path: &String) -> Result<usize, Box<dyn Error>> {
    debug!("size_bytes");
    let Some(stats) = get_stats(device_path)? else {
        return Err(Box::new(format_err!(
            "size_bytes cannot get fs stats from {}",
            device_path
        )));
    };
    let size_bytes = stats.size() as usize;
    debug!(
        "size_bytes on {}: {}B ({}GiB)",
        device_path,
        size_bytes,
        bytes_to_gib(size_bytes)
    );
    Ok(size_bytes)
}

pub fn available_bytes(device_path: &String) -> Result<usize, Box<dyn Error>> {
    debug!("available_bytes");
    let Some(stats) = get_stats(device_path)? else {
        return Err(Box::new(format_err!(
            "available_bytes cannot get fs stats from {}",
            device_path
        )));
    };
    let available_bytes = stats.available() as usize;
    debug!(
        "available_bytes on {}: {}B ({}GiB)",
        device_path,
        available_bytes,
        bytes_to_gib(available_bytes)
    );
    Ok(available_bytes)
}

pub fn used_perc(device_path: &String) -> Result<f32, Box<dyn Error>> {
    debug!("available_perc");
    let Some(stats) = get_stats(device_path)? else {
        return Err(Box::new(format_err!(
            "available_perc cannot get fs stats from {}",
            device_path
        )));
    };
    let available_perc = stats.used() as f32 / stats.size() as f32;
    debug!("available_perc on {}: {}", device_path, available_perc);
    Ok(available_perc)
}

pub fn extend_fs_max(mount_target: &String) -> Result<(), Box<dyn Error>> {
    exec("btrfs", &["filesystem", "resize", "max", mount_target])?;
    Ok(())
}

pub fn resize(mount_path: &str, new_size_bytes: usize) -> Result<(), Box<dyn Error>> {
    let new_size = format!("{}", new_size_bytes);
    exec(
        "btrfs",
        &["filesystem", "resize", new_size.as_str(), mount_path],
    )?;
    Ok(())
}
