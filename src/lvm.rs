use crate::utils::bytes_to_gib;
use crate::utils::exec;
use crate::utils::exec_bool;
use easy_error::format_err;
use log::debug;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use std::error::Error;

const LV_NAME: &str = "bsud";

pub fn lv_path(drive_name: &str) -> String {
    let drive_name = drive_name.replace('-', "--");
    format!("/dev/mapper/{}-{}", drive_name, LV_NAME)
}

pub fn get_reports() -> Result<Vec<Lvm>, Box<dyn Error>> {
    let output = exec(
        "lvm",
        &[
            "fullreport",
            "--all",
            "--units",
            "B",
            "--reportformat",
            "json",
        ],
    )?;
    let desc: JsonDesc = serde_json::from_str(output.stdout.as_str())?;
    Ok(desc.report)
}

pub fn get_report(name: &String) -> Result<Option<Lvm>, Box<dyn Error>> {
    let all_lvm = get_reports()?;
    for lvm in all_lvm {
        let Some(vg) = lvm.vg.first() else {
            continue;
        };
        if vg.vg_name == *name {
            return Ok(Some(lvm));
        }
    }
    Ok(None)
}

pub fn get_report_with_no_vg() -> Result<Option<Lvm>, Box<dyn Error>> {
    let all_lvm = get_reports()?;
    for lvm in all_lvm {
        if lvm.vg.is_empty() {
            return Ok(Some(lvm));
        }
    }
    Ok(None)
}

pub fn get_vg(name: &String) -> Result<Vg, Box<dyn Error>> {
    let Some(lvm) = get_report(name)? else {
        return Err(Box::new(format_err!("\"{}\" drive: Cannot get LVM description", name)))
    };
    let Some(vg) = lvm.vg.into_iter().next() else {
        return Err(Box::new(format_err!("\"{}\" drive: Cannot get VG description", name)))
    };
    Ok(vg)
}

pub fn get_lv(name: &String) -> Result<Lv, Box<dyn Error>> {
    let Some(lvm) = get_report(name)? else {
        return Err(Box::new(format_err!("\"{}\" drive: Cannot get LVM description", name)))
    };
    let Some(lv) = lvm.lv.into_iter().next() else {
        return Err(Box::new(format_err!("\"{}\" drive: Cannot get LV description", name)))
    };
    Ok(lv)
}

pub fn init_pv(path: &String) -> Result<(), Box<dyn Error>> {
    exec("lvm", &["pvcreate", path])?;
    Ok(())
}

pub fn vg_create(vg_name: &String, initial_pv_path: &String) -> Result<(), Box<dyn Error>> {
    exec(
        "lvm",
        &["vgcreate", "--alloc", "normal", vg_name, initial_pv_path],
    )?;
    Ok(())
}

pub fn vg_activate(activate: bool, vg_name: &String) -> Result<(), Box<dyn Error>> {
    if activate {
        exec("vgchange", &["-ay", vg_name])?;
    } else {
        exec("vgchange", &["-an", vg_name])?;
    }
    Ok(())
}

pub fn extend_vg(vg_name: &String, pv_device_path: &String) -> Result<(), Box<dyn Error>> {
    exec("lvm", &["vgextend", vg_name, pv_device_path])?;
    Ok(())
}

pub fn create_lv(vg_name: &String) -> Result<(), Box<dyn Error>> {
    exec(
        "lvm",
        &["lvcreate", "--extents", "100%FREE", "-n", LV_NAME, vg_name],
    )?;
    Ok(())
}

pub fn get_vg_size_bytes(vg_name: &String) -> Result<usize, Box<dyn Error>> {
    let mut vg = get_vg(vg_name)?;
    vg.vg_size.pop();
    let vg_size_bytes = vg.vg_size.parse::<usize>()?;
    Ok(vg_size_bytes)
}

pub fn get_lv_size_bytes(vg_name: &String) -> Result<usize, Box<dyn Error>> {
    let mut lv = get_lv(vg_name)?;
    lv.lv_size.pop();
    let lv_size_bytes = lv.lv_size.parse::<usize>()?;
    Ok(lv_size_bytes)
}

pub fn lv_extend_full(lv_path: &String) -> Result<(), Box<dyn Error>> {
    exec("lvm", &["lvextend", "--extents", "+100%FREE", lv_path])?;
    Ok(())
}

pub fn lv_activate(activate: bool, lv_name: &String) -> Result<(), Box<dyn Error>> {
    if activate {
        exec("lvchange", &["-ay", lv_name])?;
    } else {
        exec("lvchange", &["-an", lv_name])?;
    }
    Ok(())
}

pub fn vg_scan() -> Result<(), Box<dyn Error>> {
    exec("vgscan", &[])?;
    Ok(())
}

pub fn pv_move(pv_path: &String) -> Result<(), Box<dyn Error>> {
    exec_bool("lvm", &["pvmove", pv_path])?;
    Ok(())
}

pub fn pv_move_no_arg() -> Result<(), Box<dyn Error>> {
    exec_bool("lvm", &["pvmove"])?;
    Ok(())
}

pub fn lv_reduce(lv_path: &String, new_fs_size_bytes: usize) -> Result<(), Box<dyn Error>> {
    debug!(
        "lv_reduce {} of size {}B ({}GiB)",
        lv_path,
        new_fs_size_bytes,
        bytes_to_gib(new_fs_size_bytes)
    );
    exec(
        "lvm",
        &[
            "lvreduce",
            "--yes",
            "--size",
            format!("{}B", new_fs_size_bytes).as_str(),
            lv_path,
        ],
    )?;
    Ok(())
}

pub fn vg_reduce(name: &str, device_path: &str) -> Result<(), Box<dyn Error>> {
    exec("lvm", &["vgreduce", name, device_path])?;
    Ok(())
}

pub fn pv_remove(device_path: &str) -> Result<(), Box<dyn Error>> {
    exec("lvm", &["pvremove", device_path])?;
    Ok(())
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JsonDesc {
    pub report: Vec<Lvm>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lvm {
    pub vg: Vec<Vg>,
    pub pv: Vec<Pv>,
    pub lv: Vec<Lv>,
    pub pvseg: Vec<Pvseg>,
    pub seg: Vec<Seg>,
}

impl Lvm {
    pub fn devices(&self) -> Vec<String> {
        let mut all_devices = Vec::new();
        for pv in self.pv.iter() {
            all_devices.push(pv.pv_name.clone());
        }
        all_devices
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vg {
    pub vg_fmt: String,
    pub vg_uuid: String,
    pub vg_name: String,
    pub vg_attr: String,
    pub vg_permissions: String,
    pub vg_extendable: String,
    pub vg_exported: String,
    pub vg_autoactivation: String,
    pub vg_partial: String,
    pub vg_allocation_policy: String,
    pub vg_clustered: String,
    pub vg_shared: String,
    pub vg_size: String,
    pub vg_free: String,
    pub vg_sysid: String,
    pub vg_systemid: String,
    pub vg_lock_type: String,
    pub vg_lock_args: String,
    pub vg_extent_size: String,
    pub vg_extent_count: String,
    pub vg_free_count: String,
    pub max_lv: String,
    pub max_pv: String,
    pub pv_count: String,
    pub vg_missing_pv_count: String,
    pub lv_count: String,
    pub snap_count: String,
    pub vg_seqno: String,
    pub vg_tags: String,
    pub vg_profile: String,
    pub vg_mda_count: String,
    pub vg_mda_used_count: String,
    pub vg_mda_free: String,
    pub vg_mda_size: String,
    pub vg_mda_copies: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pv {
    pub pv_fmt: String,
    pub pv_uuid: String,
    pub dev_size: String,
    pub pv_name: String,
    pub pv_major: String,
    pub pv_minor: String,
    pub pv_mda_free: String,
    pub pv_mda_size: String,
    pub pv_ext_vsn: String,
    pub pe_start: String,
    pub pv_size: String,
    pub pv_free: String,
    pub pv_used: String,
    pub pv_attr: String,
    pub pv_allocatable: String,
    pub pv_exported: String,
    pub pv_missing: String,
    pub pv_pe_count: String,
    pub pv_pe_alloc_count: String,
    pub pv_tags: String,
    pub pv_mda_count: String,
    pub pv_mda_used_count: String,
    pub pv_ba_start: String,
    pub pv_ba_size: String,
    pub pv_in_use: String,
    pub pv_duplicate: String,
    pub pv_device_id: String,
    pub pv_device_id_type: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lv {
    pub lv_uuid: String,
    pub lv_name: String,
    pub lv_full_name: String,
    pub lv_path: String,
    pub lv_dm_path: String,
    pub lv_parent: String,
    pub lv_layout: String,
    pub lv_role: String,
    pub lv_initial_image_sync: String,
    pub lv_image_synced: String,
    pub lv_merging: String,
    pub lv_converting: String,
    pub lv_allocation_policy: String,
    pub lv_allocation_locked: String,
    pub lv_fixed_minor: String,
    pub lv_skip_activation: String,
    pub lv_autoactivation: String,
    pub lv_when_full: String,
    pub lv_active: String,
    pub lv_active_locally: String,
    pub lv_active_remotely: String,
    pub lv_active_exclusively: String,
    pub lv_major: String,
    pub lv_minor: String,
    pub lv_read_ahead: String,
    pub lv_size: String,
    pub lv_metadata_size: String,
    pub seg_count: String,
    pub origin: String,
    pub origin_uuid: String,
    pub origin_size: String,
    pub lv_ancestors: String,
    pub lv_full_ancestors: String,
    pub lv_descendants: String,
    pub lv_full_descendants: String,
    pub raid_mismatch_count: String,
    pub raid_sync_action: String,
    pub raid_write_behind: String,
    pub raid_min_recovery_rate: String,
    pub raid_max_recovery_rate: String,
    pub raidintegritymode: String,
    pub raidintegrityblocksize: String,
    pub integritymismatches: String,
    pub move_pv: String,
    pub move_pv_uuid: String,
    pub convert_lv: String,
    pub convert_lv_uuid: String,
    pub mirror_log: String,
    pub mirror_log_uuid: String,
    pub data_lv: String,
    pub data_lv_uuid: String,
    pub metadata_lv: String,
    pub metadata_lv_uuid: String,
    pub pool_lv: String,
    pub pool_lv_uuid: String,
    pub lv_tags: String,
    pub lv_profile: String,
    pub lv_lockargs: String,
    pub lv_time: String,
    pub lv_time_removed: String,
    pub lv_host: String,
    pub lv_modules: String,
    pub lv_historical: String,
    pub writecache_block_size: String,
    pub lv_kernel_major: String,
    pub lv_kernel_minor: String,
    pub lv_kernel_read_ahead: String,
    pub lv_permissions: String,
    pub lv_suspended: String,
    pub lv_live_table: String,
    pub lv_inactive_table: String,
    pub lv_device_open: String,
    pub data_percent: String,
    pub snap_percent: String,
    pub metadata_percent: String,
    pub copy_percent: String,
    pub sync_percent: String,
    pub cache_total_blocks: String,
    pub cache_used_blocks: String,
    pub cache_dirty_blocks: String,
    pub cache_read_hits: String,
    pub cache_read_misses: String,
    pub cache_write_hits: String,
    pub cache_write_misses: String,
    pub kernel_cache_settings: String,
    pub kernel_cache_policy: String,
    pub kernel_metadata_format: String,
    pub lv_health_status: String,
    pub kernel_discards: String,
    pub lv_check_needed: String,
    pub lv_merge_failed: String,
    pub lv_snapshot_invalid: String,
    pub vdo_operating_mode: String,
    pub vdo_compression_state: String,
    pub vdo_index_state: String,
    pub vdo_used_size: String,
    pub vdo_saving_percent: String,
    pub writecache_total_blocks: String,
    pub writecache_free_blocks: String,
    pub writecache_writeback_blocks: String,
    pub writecache_error: String,
    pub lv_attr: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pvseg {
    pub pvseg_start: String,
    pub pvseg_size: String,
    pub pv_uuid: String,
    pub lv_uuid: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seg {
    pub segtype: String,
    pub stripes: String,
    pub data_stripes: String,
    pub reshape_len: String,
    pub reshape_len_le: String,
    pub data_copies: String,
    pub data_offset: String,
    pub new_data_offset: String,
    pub parity_chunks: String,
    pub stripe_size: String,
    pub region_size: String,
    pub chunk_size: String,
    pub thin_count: String,
    pub discards: String,
    pub cache_metadata_format: String,
    pub cache_mode: String,
    pub zero: String,
    pub transaction_id: String,
    pub thin_id: String,
    pub seg_start: String,
    pub seg_start_pe: String,
    pub seg_size: String,
    pub seg_size_pe: String,
    pub seg_tags: String,
    pub seg_pe_ranges: String,
    pub seg_le_ranges: String,
    pub seg_metadata_le_ranges: String,
    pub devices: String,
    pub metadata_devices: String,
    pub seg_monitor: String,
    pub cache_policy: String,
    pub cache_settings: String,
    pub vdo_compression: String,
    pub vdo_deduplication: String,
    pub vdo_use_metadata_hints: String,
    pub vdo_minimum_io_size: String,
    pub vdo_block_map_cache_size: String,
    pub vdo_block_map_era_length: String,
    pub vdo_use_sparse_index: String,
    pub vdo_index_memory_size: String,
    pub vdo_slab_size: String,
    pub vdo_ack_threads: String,
    pub vdo_bio_threads: String,
    pub vdo_bio_rotation: String,
    pub vdo_cpu_threads: String,
    pub vdo_hash_zone_threads: String,
    pub vdo_logical_threads: String,
    pub vdo_physical_threads: String,
    pub vdo_max_discard: String,
    pub vdo_write_policy: String,
    pub vdo_header_size: String,
    pub lv_uuid: String,
}
