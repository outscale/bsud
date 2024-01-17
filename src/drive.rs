use crate::bsu::Bsu;
use crate::config::{self, Config, ConfigFileDrive, DriveTarget, VM_ID};
use crate::fs;
use crate::lvm;
use crate::utils::{bytes_to_gib, bytes_to_gib_rounded, gib_to_bytes};
use datetime::{Duration, Instant};
use easy_error::format_err;
use log::info;
use log::{debug, error};
use std::cmp::Ordering;
use std::cmp::{max, min};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::sleep;
use std::time;
use threadpool::ThreadPool;

const RECONCILE_COOLDOWN_S: u64 = 30;
const DEFAULT_INITIAL_DISK_GIB: usize = 10;
const DEFAULT_MAX_DISKS: usize = 10;
const DEFAULT_MAX_USED_PERC: usize = 85;
const DEFAULT_MIN_USED_PERC: usize = 40;
const DEFAULT_SCALE_FACTOR_PERC: usize = 20;
const DEFAULT_DISK_TYPE: config::DiskType = config::DiskType::Gp2;
// https://docs.outscale.com/api#createvolume
const MAX_BSU_SIZE_GIB: usize = 14901;

type DriveName = String;

#[derive(Debug, Default)]
pub struct Drives {
    drives_cmd: HashMap<DriveName, Sender<DriveCmd>>,
    drives_threads: ThreadPool,
}

impl Drives {
    pub fn run(config: Config) -> Result<Drives, Box<dyn Error>> {
        let mut drives_cmd = HashMap::<DriveName, Sender<DriveCmd>>::new();
        let mut drive_list = Vec::<Drive>::new();

        for drive_config in config.drives {
            let name = drive_config.name.clone();

            let (sender, receiver) = channel::<DriveCmd>();
            let drive = Drive::new(drive_config, receiver);
            drives_cmd.insert(name.clone(), sender);
            drive_list.push(drive);
        }

        for (sender, drive) in Drives::discover_local_drives()? {
            let name: String = drive.name.clone();
            if drives_cmd.get(&name).is_some() {
                continue;
            }
            drives_cmd.insert(name.clone(), sender);
            drive_list.push(drive);
        }

        let drives_threads = ThreadPool::new(drive_list.len());
        for mut drive in drive_list {
            drives_threads.execute(move || drive.run());
        }

        Ok(Drives {
            drives_cmd,
            drives_threads,
        })
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        for (name, sender) in self.drives_cmd.iter() {
            info!("asking drive {} to stop", name);
            sender.send(DriveCmd::Stop)?;
        }
        info!("waiting for drives to stop");
        self.drives_threads.join();
        info!("all drives stopped");
        Ok(())
    }

    pub fn discover_local_drives() -> Result<DriveDiscovery, Box<dyn Error>> {
        // TODO
        Ok(vec![])
    }
}

type DriveDiscovery = Vec<(Sender<DriveCmd>, Drive)>;
type DevicePath = String;

#[derive(Debug)]
pub enum DriveCmd {
    Stop,
}

#[derive(Debug)]
pub struct Drive {
    last_reconcile: Instant,
    all_bsu: Vec<Bsu>,
    drive_cmd: Receiver<DriveCmd>,
    exit: bool,
    pv_to_be_initialized: Vec<DevicePath>,
    pv_to_add_to_vg: Vec<DevicePath>,
    pub name: String,
    pub target: DriveTarget,
    pub mount_path: String,
    pub disk_type: config::DiskType,
    pub disk_iops_per_gib: Option<usize>,
    pub max_total_size_gib: Option<usize>,
    pub initial_size_gib: usize,
    pub max_bsu_count: usize,
    pub max_used_space_perc: f32,
    pub min_used_space_perc: f32,
    pub disk_scale_factor_perc: f32,
}

impl Drive {
    pub fn new(config: ConfigFileDrive, drive_cmd: Receiver<DriveCmd>) -> Self {
        Drive {
            last_reconcile: Instant::now() - Duration::of(RECONCILE_COOLDOWN_S as i64),
            all_bsu: Vec::default(),
            drive_cmd,
            exit: false,
            pv_to_be_initialized: Vec::new(),
            pv_to_add_to_vg: Vec::new(),
            name: config.name,
            target: config.target,
            mount_path: config.mount_path,
            disk_type: config.disk_type.unwrap_or(DEFAULT_DISK_TYPE),
            initial_size_gib: config.initial_size_gib.unwrap_or(DEFAULT_INITIAL_DISK_GIB),
            max_bsu_count: config.max_bsu_count.unwrap_or(DEFAULT_MAX_DISKS),
            max_used_space_perc: config.max_used_space_perc.unwrap_or(DEFAULT_MAX_USED_PERC) as f32
                / 100.0,
            min_used_space_perc: config.min_used_space_perc.unwrap_or(DEFAULT_MIN_USED_PERC) as f32
                / 100.0,
            disk_scale_factor_perc: config
                .disk_scale_factor_perc
                .unwrap_or(DEFAULT_SCALE_FACTOR_PERC) as f32
                / 100.0,
            disk_iops_per_gib: config.disk_iops_per_gib,
            max_total_size_gib: config.max_total_size_gib,
        }
    }

    pub fn run(&mut self) {
        loop {
            if Instant::now().seconds() - self.last_reconcile.seconds()
                <= RECONCILE_COOLDOWN_S as i64
            {
                sleep(time::Duration::from_millis(10));
                if self.early_exit().is_err() {
                    break;
                }
                continue;
            }
            if let Err(err) = self.reconcile() {
                error!("\"{}\" drive: {}", self.name, err);
            } else {
                info!("\"{}\" drive: reconcile loop over with success", self.name);
            }
            self.last_reconcile = Instant::now();
            if self.exit {
                break;
            }
        }
        info!("\"{}\" drive: stopped", self.name);
    }

    pub fn early_exit(&mut self) -> Result<(), Box<dyn Error>> {
        if let Ok(cmd) = self.drive_cmd.try_recv() {
            info!("\"{}\" drive received {:?} command", self.name, cmd);
            match cmd {
                DriveCmd::Stop => {
                    self.exit = true;
                    return Err(Box::new(format_err!(
                        "\"{}\" drive: early exit due to drive stop",
                        self.name
                    )));
                }
            };
        }
        Ok(())
    }

    pub fn reconcile(&mut self) -> Result<(), Box<dyn Error>> {
        info!(
            "\"{}\" drive: entering {:?} drive target",
            self.name, self.target
        );
        info!(
            "\"{}\" drive: start reconcile {}",
            self.name,
            self.target.to_string()
        );
        match self.target {
            DriveTarget::Online => self.reconcile_online(),
            DriveTarget::Offline => self.reconcile_offline(),
            DriveTarget::Delete => self.reconcile_delete(),
        }
    }

    pub fn reconcile_offline(&mut self) -> Result<(), Box<dyn Error>> {
        self.early_exit()?;
        while self.is_fs_mounted()? {
            self.early_exit()?;
            self.fs_umount()?;
        }

        self.disable_lv().ok();
        self.disable_vg().ok();

        self.early_exit()?;
        self.fetch_all_drive_bsu()?;
        if self.bsu_count() == 0 {
            return Ok(());
        }

        while self.are_bsu_attached()? {
            self.early_exit()?;
            self.bsu_detach_all_from_this_vm()?;
            self.early_exit()?;
            self.fetch_all_drive_bsu()?;
            self.early_exit()?;
        }
        self.vg_scan().ok();
        Ok(())
    }

    pub fn reconcile_delete(&mut self) -> Result<(), Box<dyn Error>> {
        self.reconcile_offline()?;
        self.delete_all_bsu()?;
        Ok(())
    }

    pub fn reconcile_online(&mut self) -> Result<(), Box<dyn Error>> {
        'start_again: loop {
            debug!("\"{}\" drive: reconcile online loop again", self.name);
            self.early_exit()?;
            self.crash_resume()?;

            self.early_exit()?;
            self.fetch_all_drive_bsu()?;

            self.early_exit()?;
            while !self.are_bsu_attached()? {
                self.bsu_attach_missing()?;
                self.fetch_all_drive_bsu()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            if self.bsu_count() == 0 {
                self.create_initial_bsu()?;
                continue 'start_again;
            }

            self.early_exit()?;
            while !self.are_pv_initialized()? {
                self.pv_initialize_missing()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            self.vg_scan().ok();

            self.early_exit()?;
            while !self.is_vg_created()? {
                self.vg_create()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            self.enable_vg().ok();

            self.early_exit()?;
            while !self.is_vg_extended()? {
                self.vg_extend()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            while !self.is_lv_created()? {
                self.lv_create()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            self.enable_lv().ok();

            self.early_exit()?;
            self.lv_extend()?;

            self.early_exit()?;
            while !self.is_fs_formated()? {
                self.fs_format()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            while !self.is_mount_path_created() {
                self.create_mount_path()?;
            }

            self.early_exit()?;
            while !self.is_fs_mounted()? {
                self.fs_mount()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            while !self.is_fs_extended()? {
                self.fs_extend()?;
                self.early_exit()?;
            }

            self.early_exit()?;
            if self.is_drive_reached_max_attached_bsu()? {
                self.remove_smallest_bsu()?;
                self.early_exit()?;
                continue 'start_again;
            }

            self.early_exit()?;
            if self.is_drive_low_space_left()? {
                if self.is_max_space_reached() {
                    return Ok(());
                }
                if !self.is_drive_reached_max_attached_bsu_minus_one()?
                    && !self.is_drive_contains_smallest_bsu()
                {
                    self.create_smaller_bsu()?;
                } else {
                    self.create_larger_bsu()?;
                }
                continue 'start_again;
            }

            self.early_exit()?;
            if self.is_drive_high_space_left()? {
                if self.bsu_count() > 1 {
                    self.remove_largest_bsu()?;
                } else {
                    if self.has_minimal_size() {
                        return Ok(());
                    }
                    self.create_ideal_bsu()?;
                }
                self.early_exit()?;
                continue 'start_again;
            }
            return Ok(());
        }
    }

    pub fn crash_resume(&mut self) -> Result<(), Box<dyn Error>> {
        // Run pvmove alone to restart eventual pvmove actions
        // https://www.man7.org/linux/man-pages/man8/pvmove.8.html
        lvm::pv_move_no_arg()?;
        Ok(())
    }

    pub fn fetch_all_drive_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: fetch all bsu", self.name);
        self.all_bsu = Bsu::fetch_drive(&self.name)?;
        info!(
            "\"{}\" drive: fetched {} BSU",
            self.name,
            self.all_bsu.len()
        );
        Ok(())
    }

    pub fn are_bsu_attached(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut ret = true;
        debug!("\"{}\" drive: are bsu attached ?", self.name);
        let vm_id = VM_ID.try_read()?;
        for bsu in self.all_bsu.iter() {
            let Some(bsu_vm_id) = &bsu.vm_id else {
                debug!(
                    "\"{}\" drive: BSU id {} not attached to any VM",
                    self.name, bsu.id
                );
                ret = false;
                continue;
            };
            if *bsu_vm_id != *vm_id {
                debug!(
                    "\"{}\" drive: BSU id {} is attached to vm {} instead of vm {}",
                    self.name, bsu.id, bsu_vm_id, vm_id
                );
                ret = false;
                continue;
            }
            let Some(device_name) = &bsu.device_path else {
                debug!(
                    "\"{}\" drive: BSU id {} does not have a device path",
                    self.name, bsu.id
                );
                ret = false;
                continue;
            };
            let device_path = Path::new(device_name);
            if !device_path.exists() {
                debug!(
                    "\"{}\" drive: BSU id {} seems not to exist yet on {}",
                    self.name, bsu.id, device_name
                );
                ret = false;
                continue;
            }
            debug!(
                "\"{}\" drive: bsu id {} of size {}B ({}GiB) is attached",
                self.name,
                bsu.id,
                bsu.size_bytes,
                bytes_to_gib(bsu.size_bytes)
            );
        }
        info!("\"{}\" drive: are bsu attached ? -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn bsu_attach_missing(&mut self) -> Result<(), Box<dyn Error>> {
        let vm_id: String = VM_ID.try_read()?.clone();
        let bsus: Vec<Bsu> = self
            .all_bsu
            .iter()
            .filter(|bsu| bsu.vm_id.is_none())
            .cloned()
            .collect();
        Bsu::multiple_attach(&vm_id, &bsus)
    }

    pub fn bsu_detach_all_from_this_vm(&mut self) -> Result<(), Box<dyn Error>> {
        info!(
            "\"{}\" drive: detach all {} BSU",
            self.name,
            self.all_bsu.len()
        );
        Bsu::multiple_detach(&self.all_bsu)
    }

    pub fn delete_all_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        info!(
            "\"{}\" drive: delete all {} BSU",
            self.name,
            self.all_bsu.len()
        );
        for bsu in self.all_bsu.iter() {
            bsu.delete()?;
        }
        Ok(())
    }

    pub fn bsu_count(&mut self) -> usize {
        let count = self.all_bsu.len();
        debug!("\"{}\" drive: bsu count = {}", self.name, count);
        count
    }

    pub fn create_initial_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: create initial BSU", self.name);
        Bsu::create_gib(
            &self.name,
            &self.disk_type,
            self.disk_iops_per_gib,
            self.initial_size_gib,
        )
    }

    pub fn are_pv_initialized(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut ret = true;
        self.pv_to_be_initialized.clear();
        let mut found_devices = HashSet::<String>::new();
        if let Some(report_with_no_vg) = lvm::get_report_with_no_vg()? {
            for device in report_with_no_vg.devices() {
                found_devices.insert(device);
            }
        }
        if let Some(report) = lvm::get_report(&self.name)? {
            for device in report.devices() {
                found_devices.insert(device);
            }
        }
        for bsu in self.all_bsu.iter() {
            let Some(device_path) = &bsu.device_path else {
                error!(
                    "\"{}\" drive: BSU {} should have loca path, please report error",
                    self.name, bsu.id
                );
                continue;
            };
            if !found_devices.contains(device_path) {
                info!(
                    "\"{}\" drive: BSU {} ({}) seems not to be pv initialized",
                    self.name, bsu.id, device_path
                );
                self.pv_to_be_initialized.push(device_path.clone());
                ret = false;
            }
        }
        info!("\"{}\" drive: are pv initialized -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn pv_initialize_missing(&mut self) -> Result<(), Box<dyn Error>> {
        for device in self.pv_to_be_initialized.iter() {
            lvm::init_pv(device)?;
        }
        Ok(())
    }

    pub fn is_vg_created(&mut self) -> Result<bool, Box<dyn Error>> {
        let lvm = lvm::get_report(&self.name)?;
        info!(
            "\"{}\" drive: is vg created -> {}",
            self.name,
            lvm.is_some()
        );
        Ok(lvm.is_some())
    }

    pub fn vg_create(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: create vg", self.name);
        let mut found_devices = HashSet::<String>::new();
        if let Some(report_with_no_vg) = lvm::get_report_with_no_vg()? {
            for device in report_with_no_vg.devices() {
                found_devices.insert(device);
            }
        }
        for bsu in self.all_bsu.iter() {
            let Some(device_path) = &bsu.device_path else {
                error!(
                    "\"{}\" drive: BSU {} should have local path, please report error",
                    self.name, bsu.id
                );
                continue;
            };
            if found_devices.contains(device_path) {
                return lvm::vg_create(&self.name, device_path);
            }
        }
        Err(Box::new(format_err!(
            "\"{}\" drive: no PV found to init VG, please report this error",
            self.name
        )))
    }

    pub fn is_vg_extended(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut ret = true;
        self.pv_to_add_to_vg.clear();
        let mut found_devices = HashSet::<String>::new();
        if let Some(report_with_no_vg) = lvm::get_report_with_no_vg()? {
            for device in report_with_no_vg.devices() {
                found_devices.insert(device);
            }
        }
        for bsu in self.all_bsu.iter() {
            let Some(device_path) = &bsu.device_path else {
                error!(
                    "\"{}\" drive: BSU {} should have local path, please report error",
                    self.name, bsu.id
                );
                continue;
            };
            if found_devices.contains(device_path) {
                info!(
                    "\"{}\" drive: pv {} can be added to vg",
                    self.name, device_path
                );
                self.pv_to_add_to_vg.push(device_path.clone());
                ret = false;
            }
        }
        info!("\"{}\" drive: is vg extended -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn vg_extend(&mut self) -> Result<(), Box<dyn Error>> {
        for pv_device_path in self.pv_to_add_to_vg.iter() {
            lvm::extend_vg(&self.name, pv_device_path)?;
        }
        Ok(())
    }

    pub fn is_lv_created(&mut self) -> Result<bool, Box<dyn Error>> {
        let Some(lvm) = lvm::get_report(&self.name)? else {
            return Err(Box::new(format_err!(
                "\"{}\" drive: lvm details cannot be found, please report issue",
                self.name
            )));
        };
        let Some(_lv) = lvm.lv.into_iter().next() else {
            debug!("\"{}\" drive: is lv created -> false", self.name);
            return Ok(false);
        };
        info!("\"{}\" drive: is lv created -> true", self.name);
        Ok(true)
    }

    pub fn lv_create(&mut self) -> Result<(), Box<dyn Error>> {
        lvm::create_lv(&self.name)
    }

    pub fn lv_extend(&mut self) -> Result<(), Box<dyn Error>> {
        let vg_size = lvm::get_vg_size_bytes(&self.name)?;
        let lv_size = lvm::get_lv_size_bytes(&self.name)?;
        match vg_size.cmp(&lv_size) {
            Ordering::Greater => {
                debug!("\"{}\" drive: lv can be extended", self.name);
                let lv_path = lvm::lv_path(&self.name);
                lvm::lv_extend_full(&lv_path)?;
            }
            Ordering::Equal => debug!("\"{}\" drive: lv fit vg", self.name),
            Ordering::Less => {
                return Err(Box::new(format_err!(
                    "\"{}\" drive: vg_size ({}) < lv_size ({})",
                    self.name,
                    vg_size,
                    lv_size
                )));
            }
        };
        Ok(())
    }

    pub fn enable_lv(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: disabling lv {}", self.name, self.name);
        lvm::lv_activate(true, &self.name)
    }

    pub fn disable_lv(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: disabling lv {}", self.name, self.name);
        lvm::lv_activate(false, &self.name)
    }

    pub fn enable_vg(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: enabling vg {}", self.name, self.name);
        lvm::vg_activate(true, &self.name)
    }

    pub fn disable_vg(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: disabling vg {}", self.name, self.name);
        lvm::vg_activate(false, &self.name)
    }

    pub fn vg_scan(&self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: vgscan", self.name);
        lvm::vg_scan()
    }

    pub fn is_fs_formated(&mut self) -> Result<bool, Box<dyn Error>> {
        let lv_path = lvm::lv_path(&self.name);
        let ret = fs::device_seems_formated(&lv_path)?;
        info!("\"{}\" drive: is fs formated -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn fs_format(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: fs format", self.name);
        let lv_path = lvm::lv_path(&self.name);
        fs::format(&lv_path)
    }

    pub fn is_mount_path_created(&mut self) -> bool {
        let ret = fs::is_folder(&self.mount_path);
        debug!(
            "\"{}\" drive: is mount target created ? -> {}",
            self.name, ret
        );
        ret
    }

    pub fn create_mount_path(&mut self) -> Result<(), Box<dyn Error>> {
        debug!(
            "\"{}\" drive: try creating folder in {}",
            self.name, self.mount_path
        );
        fs::create_folder(&self.mount_path)
    }

    pub fn is_fs_mounted(&mut self) -> Result<bool, Box<dyn Error>> {
        let lv_path = lvm::lv_path(&self.name);
        let ret = fs::is_mounted(&lv_path, &self.mount_path)?;
        info!("\"{}\" drive: is fs mounted ? -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn fs_mount(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: fs mount", self.name);
        let lv_path = lvm::lv_path(&self.name);
        fs::mount(&lv_path, &self.mount_path)
    }

    pub fn fs_umount(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: fs umount", self.name);
        let lv_path = lvm::lv_path(&self.name);
        fs::umount(&lv_path)
    }

    pub fn is_fs_extended(&mut self) -> Result<bool, Box<dyn Error>> {
        let lv_size = lvm::get_lv_size_bytes(&self.name)?;
        let lv_path = lvm::lv_path(&self.name);
        let fs_size = fs::size_bytes(&lv_path)?;
        debug!(
            "\"{}\" drive: lv size: {}B ({}GiB), fs size: {}B ({}GiB)",
            self.name,
            lv_size,
            bytes_to_gib(lv_size),
            fs_size,
            bytes_to_gib(fs_size)
        );
        let ret = match fs_size.cmp(&lv_size) {
            Ordering::Equal => true,
            Ordering::Less => false,
            Ordering::Greater => {
                return Err(Box::new(format_err!(
                    "\"{}\" drive: fs_size > lv_size",
                    self.name
                )))
            }
        };
        info!("\"{}\" drive: is fs extended ? -> {}", self.name, ret);
        Ok(ret)
    }

    pub fn fs_extend(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: fs extend", self.name);
        fs::extend_fs_max(&self.mount_path)
    }

    pub fn is_drive_reached_max_attached_bsu(&mut self) -> Result<bool, Box<dyn Error>> {
        let count = self.bsu_count();
        let ret = count >= self.max_bsu_count;
        info!(
            "\"{}\" drive: is drive reached max attached BSU: (count: {}, max: {}) -> {}",
            self.name, count, self.max_bsu_count, ret
        );
        Ok(ret)
    }

    pub fn is_drive_reached_max_attached_bsu_minus_one(&mut self) -> Result<bool, Box<dyn Error>> {
        let ret = self.bsu_count() == self.max_bsu_count - 1;
        info!(
            "\"{}\" drive: is drive reached max attached BSU minus ONE (count: {}, max: {}) -> {}",
            self.name,
            self.all_bsu.len(),
            self.max_bsu_count,
            ret
        );
        Ok(ret)
    }

    pub fn is_drive_contains_smallest_bsu(&mut self) -> bool {
        let ret = self.smallest_bsu().size_gib <= self.initial_size_gib;
        debug!(
            "\"{}\" drive: is_drive_contains_smallest_bsu ? -> {}",
            self.name, ret
        );
        ret
    }

    pub fn remove_smallest_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: remove smallest BSU", self.name);
        let bsu = self.smallest_bsu();
        self.remove_bsu(&bsu)
    }

    pub fn is_drive_low_space_left(&mut self) -> Result<bool, Box<dyn Error>> {
        let lv_path = lvm::lv_path(&self.name);
        let usage_per = fs::used_perc(&lv_path)?;
        let ret = usage_per >= self.max_used_space_perc;
        debug!(
            "\"{}\" drive: used space perc: {}, max_used_space_perc: {}",
            self.name, usage_per, self.max_used_space_perc
        );
        info!(
            "\"{}\" drive: is drive low space left -> {}",
            self.name, ret
        );
        Ok(ret)
    }

    pub fn is_max_space_reached(&mut self) -> bool {
        let Some(max_total_size_gib) = self.max_total_size_gib else {
            return false;
        };
        let total_gib = self.all_bsu_size_gib();
        let ret = total_gib >= max_total_size_gib;
        info!(
            "\"{}\" drive: is max space reached -> {} ({}/{}Gib)",
            self.name, ret, total_gib, max_total_size_gib
        );
        ret
    }

    pub fn all_bsu_size_gib(&self) -> usize {
        let mut total_size: usize = 0;
        for bsu in self.all_bsu.iter() {
            total_size += bsu.size_bytes;
        }
        bytes_to_gib_rounded(total_size)
    }

    pub fn create_larger_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: create larger BSU", self.name);
        let largest_size_gib = self.largest_bsu().size_gib as f32;
        let new_bsu_size_gib =
            (largest_size_gib + largest_size_gib * self.disk_scale_factor_perc).ceil() as usize;
        let final_bsu_size = min(MAX_BSU_SIZE_GIB, new_bsu_size_gib);
        Bsu::create_gib(
            &self.name,
            &self.disk_type,
            self.disk_iops_per_gib,
            final_bsu_size,
        )
    }

    pub fn create_smaller_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        debug!("\"{}\" drive: create smaller BSU", self.name);
        let largest_size_gib = self.smallest_bsu().size_gib as f32;
        let new_bsu_size_gib =
            (largest_size_gib - largest_size_gib * self.disk_scale_factor_perc).ceil() as usize;
        let final_bsu_size = max(self.initial_size_gib, new_bsu_size_gib);
        Bsu::create_gib(
            &self.name,
            &self.disk_type,
            self.disk_iops_per_gib,
            final_bsu_size,
        )
    }

    pub fn largest_bsu(&self) -> Bsu {
        let mut largest_bsu_size = 0;
        let mut largest_bsu: Option<&Bsu> = None;
        for bsu in self.all_bsu.iter() {
            if bsu.size_bytes > largest_bsu_size {
                largest_bsu = Some(bsu);
                largest_bsu_size = bsu.size_bytes;
            }
        }
        largest_bsu
            .expect("largest BSU should exit, please report this")
            .clone()
    }

    pub fn smallest_bsu(&self) -> Bsu {
        let mut smallest_bsu_size = usize::MAX;
        let mut smallest_bsu: Option<&Bsu> = None;
        for bsu in self.all_bsu.iter() {
            if bsu.size_bytes < smallest_bsu_size {
                smallest_bsu = Some(bsu);
                smallest_bsu_size = bsu.size_bytes;
            }
        }
        smallest_bsu
            .expect("smallest BSU should exit, please report this")
            .clone()
    }

    pub fn is_drive_high_space_left(&mut self) -> Result<bool, Box<dyn Error>> {
        let lv_path = lvm::lv_path(&self.name);
        let usage_per = fs::used_perc(&lv_path)?;
        let ret = usage_per <= self.min_used_space_perc;
        debug!(
            "\"{}\" drive: used space perc: {}, low space perc: {}",
            self.name, usage_per, self.min_used_space_perc
        );
        info!(
            "\"{}\" drive: is drive high space left -> {}",
            self.name, ret
        );
        Ok(ret)
    }

    pub fn has_minimal_size(&self) -> bool {
        let total_size_gib = self.all_bsu_size_gib();
        let ret = total_size_gib == self.initial_size_gib;
        info!("\"{}\" drive: has minimal size -> {}", self.name, ret);
        ret
    }

    pub fn ideal_size_bytes(&mut self) -> Result<usize, Box<dyn Error>> {
        let lv_path = lvm::lv_path(&self.name);
        let used_size_bytes = fs::used_bytes(&lv_path)? as f32;
        let middle_perc = (self.min_used_space_perc + self.max_used_space_perc) / 2.0;
        let ideal_size_bytes = (used_size_bytes / middle_perc).ceil() as usize;
        let ideal_size_bytes = max(ideal_size_bytes, gib_to_bytes(self.initial_size_gib));
        let ideal_size_bytes = min(ideal_size_bytes, fs::size_bytes(&lv_path)?);
        Ok(ideal_size_bytes)
    }

    pub fn create_ideal_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        let ideal_size_gib = bytes_to_gib_rounded(self.ideal_size_bytes()?);
        info!(
            "\"{}\" drive: create fit BSU of size {}GiB",
            self.name, ideal_size_gib
        );
        Bsu::create_gib(
            &self.name,
            &self.disk_type,
            self.disk_iops_per_gib,
            ideal_size_gib,
        )?;
        Ok(())
    }

    pub fn remove_largest_bsu(&mut self) -> Result<(), Box<dyn Error>> {
        info!("\"{}\" drive: remove largest BSU", self.name);
        let bsu = self.largest_bsu();
        self.remove_bsu(&bsu)
    }

    pub fn remove_bsu(&mut self, bsu: &Bsu) -> Result<(), Box<dyn Error>> {
        info!(
            "removing BSU {} of size {}B ({}GiB)",
            bsu.id,
            bsu.size_bytes,
            bytes_to_gib(bsu.size_bytes)
        );
        let lv_path = lvm::lv_path(&self.name);
        let free_space_bytes = fs::available_bytes(&lv_path)?;
        if free_space_bytes < bsu.size_bytes {
            return Err(Box::new(format_err!(
                "\"{}\" drive: cannot remove BSU. free space left: {}B ({}GiB), bsu size to remove: {} ({}GiB)",
                self.name,
                free_space_bytes,
                bytes_to_gib(free_space_bytes),
                bsu.size_bytes,
                bytes_to_gib(bsu.size_bytes)
            )));
        }
        let Some(device_path) = &bsu.device_path else {
            return Err(Box::new(format_err!(
                "\"{}\" drive: cannot find device path for BSU {}",
                self.name,
                bsu.id
            )));
        };

        let ideal_size_bytes = self.ideal_size_bytes()?;
        let fs_size_bytes = fs::size_bytes(&lv_path)?;
        let largest_possible_new_fs_size = fs_size_bytes - bsu.size_bytes;
        // trying (when possible) to lower more than required to delete the BSU will drastically help pvmove not to move useless fs data.
        let new_fs_size_bytes = min(largest_possible_new_fs_size, ideal_size_bytes);

        debug!(
            "\"{}\" drive: resising fs & lv to {}B ({}GiB)",
            self.name,
            new_fs_size_bytes,
            bytes_to_gib(new_fs_size_bytes)
        );
        debug!(
            "\"{}\" drive: ideal_size_bytes was {}B ({}GiB)",
            self.name,
            ideal_size_bytes,
            bytes_to_gib(ideal_size_bytes)
        );
        debug!(
            "\"{}\" drive: largest_possible_new_fs_size was {}B ({}GiB)",
            self.name,
            largest_possible_new_fs_size,
            bytes_to_gib(largest_possible_new_fs_size)
        );

        fs::resize(&self.mount_path, new_fs_size_bytes)?;
        let lv_path = lvm::lv_path(&self.name);
        lvm::lv_reduce(&lv_path, new_fs_size_bytes)?;
        lvm::pv_move(device_path)?;
        lvm::vg_reduce(&self.name, device_path)?;
        lvm::pv_remove(device_path)?;
        // Once pv moved, be sure we can expand back lv and fs.
        self.lv_extend()?;
        self.fs_extend()?;

        bsu.detach()?;
        bsu.delete()?;
        Ok(())
    }
}
