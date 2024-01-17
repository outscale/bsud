use crate::config::{DiskType, CLOUD_CONFIG, SUBREGION, VM_ID};
use crate::utils::gib_to_bytes;
use easy_error::format_err;
use log::{debug, error};
use outscale_api::apis::tag_api::create_tags;
use outscale_api::apis::volume_api::{
    create_volume, delete_volume, link_volume, read_volumes, unlink_volume,
};
use outscale_api::models::{
    CreateTagsRequest, CreateVolumeRequest, DeleteVolumeRequest, FiltersVolume, LinkVolumeRequest,
    ReadVolumesRequest, ResourceTag, UnlinkVolumeRequest, Volume,
};
use std::error::Error;
use std::path::PathBuf;

use datetime::{Duration, Instant};
use lazy_static::lazy_static;
use std::sync::Mutex;
use std::thread::sleep;
use std::time;

const API_LIMITER_S: u64 = 3;
const BSU_TAG_KEY: &str = "osc.bsud.drive-name";
const MAX_IOPS_PER_VOLUMES: usize = 13000;
const DEFAULT_IO1_IOPS_PER_GB: usize = 100;

lazy_static! {
    pub static ref API_LIMITER: Mutex<Instant> =
        Mutex::new(Instant::now() - Duration::of(API_LIMITER_S as i64));
}

#[derive(Debug, Default, Clone)]
pub struct Bsu {
    pub vm_id: Option<String>,
    pub drive_name: String,
    pub id: String,
    pub size_bytes: usize,
    pub size_gib: usize,
    pub device_path: Option<String>,
}

impl Bsu {
    pub fn new(volume: &Volume) -> Result<Self, Box<dyn Error>> {
        let Some(bsu_id) = volume.volume_id.clone() else {
            return Err(Box::new(format_err!(
                "BSU {:?} does not have an id",
                volume
            )));
        };
        let Some(bsu_size_gib) = volume.size else {
            return Err(Box::new(format_err!(
                "BSU {:?} does not have a size",
                volume
            )));
        };
        let vm_id = Bsu::get_drive_linked_vm_id(volume);
        let Some(drive_name) = Bsu::get_drive_name(volume) else {
            Err(format_err!(
                "Cannot extract drive name from BSU id {}",
                bsu_id
            ))?
        };
        let device_path = Bsu::get_drive_device_path(volume);

        Ok(Bsu {
            vm_id,
            drive_name,
            id: bsu_id,
            size_bytes: gib_to_bytes(bsu_size_gib as usize),
            size_gib: bsu_size_gib as usize,
            device_path,
        })
    }

    fn get_drive_linked_vm_id(volume: &Volume) -> Option<String> {
        let Some(linked_volumes) = &volume.linked_volumes else {
            return None;
        };
        for linked_volume in linked_volumes {
            let Some(state) = &linked_volume.state else {
                continue;
            };
            let Some(vm_id) = &linked_volume.vm_id else {
                continue;
            };
            match state.as_str() {
                "attaching" | "attached" => return Some(vm_id.clone()),
                _ => return None,
            };
        }
        None
    }

    fn get_drive_name(volume: &Volume) -> Option<String> {
        let Some(tags) = &volume.tags else {
            return None;
        };
        for tag in tags {
            if tag.key == *BSU_TAG_KEY.to_string() {
                return Some(tag.value.clone());
            }
        }
        None
    }

    fn get_drive_device_path(volume: &Volume) -> Option<String> {
        let Some(linked_volumes) = &volume.linked_volumes else {
            return None;
        };
        linked_volumes.iter().next()?.device_name.clone()
    }

    pub fn fetch_drive(drive_name: &String) -> Result<Vec<Bsu>, Box<dyn Error>> {
        debug!("\"{}\" drive: fetching all bsu", drive_name);
        api_limiter()?;
        let mut request = ReadVolumesRequest::new();
        let mut filter = FiltersVolume::default();
        let tag = format!("{}={}", BSU_TAG_KEY, drive_name);
        filter.tags = Some(vec![tag]);
        filter.volume_states = Some(vec![
            "creating".to_string(),
            "available".to_string(),
            "in-use".to_string(),
        ]);
        request.filters = Some(Box::new(filter));
        let response = read_volumes(&*CLOUD_CONFIG.read()?, Some(request));
        if response.is_err() {
            error!("read volume response: {:?}", response);
        }
        let response = response?;
        let volumes = response.volumes.unwrap_or_default();
        // Check state filtering
        let volumes: Vec<Volume> = volumes
            .into_iter()
            .filter(|vol| {
                let Some(state) = &vol.state else {
                    return false;
                };
                matches!(state.as_str(), "creating" | "available" | "in-use")
            })
            .collect();
        let bsu_list = volumes.iter().map(Bsu::new).collect();
        bsu_list
    }

    pub fn detach(&self) -> Result<(), Box<dyn Error>> {
        debug!("detaching BSU {} on vm {:?}", self.id, self.vm_id);
        api_limiter()?;
        let request = UnlinkVolumeRequest::new(self.id.clone());
        let response = unlink_volume(&*CLOUD_CONFIG.read()?, Some(request));
        if response.is_err() {
            error!("unlink volume response: {:?}", response);
            response?;
        }
        Bsu::wait_state(&self.id, "available")?;
        Ok(())
    }

    pub fn multiple_attach(vm_id: &String, bsus: &Vec<Bsu>) -> Result<(), Box<dyn Error>> {
        for bsu in bsus {
            debug!("attaching BSU {} on vm {:?}", bsu.id, vm_id);
            api_limiter()?;
            let Some(device_name) = Bsu::find_next_available_device() else {
                return Err(Box::new(format_err!(
                    "cannot find available device to attach {} BSU on {} VM",
                    bsu.id,
                    vm_id
                )));
            };
            let request = LinkVolumeRequest::new(device_name, vm_id.clone(), bsu.id.clone());
            let response = link_volume(&*CLOUD_CONFIG.read()?, Some(request));
            if response.is_err() {
                error!("link volume response: {:?}", response);
                response?;
            }
        }
        Bsu::wait_states(bsus, "in-use")?;
        Ok(())
    }

    pub fn multiple_detach(bsus: &Vec<Bsu>) -> Result<(), Box<dyn Error>> {
        let vm_id: String = VM_ID.try_read()?.clone();
        let mut unlinked_volumes = Vec::new();
        for bsu in bsus {
            debug!("detaching BSU {} on vm {}", bsu.id, vm_id);
            let Some(ref bsu_vm_id) = bsu.vm_id else {
                debug!(
                    "BSU id {} seems not to be attached, ignore detaching",
                    bsu.id
                );
                continue;
            };
            if vm_id != *bsu_vm_id {
                debug!(
                    "BSU {} id seems attached to vm {}, not on vm {}, ignore detaching",
                    bsu.id, bsu_vm_id, vm_id
                );
                continue;
            }
            api_limiter()?;
            let request = UnlinkVolumeRequest::new(bsu.id.clone());
            let response = unlink_volume(&*CLOUD_CONFIG.read()?, Some(request));
            if response.is_err() {
                error!("unlink volume response: {:?}", response);
                response?;
            }
            unlinked_volumes.push(bsu.clone());
        }
        Bsu::wait_states(&unlinked_volumes, "available")?;
        Ok(())
    }

    pub fn delete(&self) -> Result<(), Box<dyn Error>> {
        debug!("deleting BSU {}", self.id);
        api_limiter()?;
        let request = DeleteVolumeRequest::new(self.id.clone());
        let response = delete_volume(&*CLOUD_CONFIG.read()?, Some(request));
        if response.is_err() {
            error!("delete volume response: {:?}", response);
            response?;
        }
        Ok(())
    }

    pub fn wait_state(bsu_id: &String, desired_state: &str) -> Result<(), Box<dyn Error>> {
        loop {
            let volume_state = Bsu::get_state(bsu_id)?;
            debug!(
                "volume {} state: {}, desired state: {}",
                bsu_id, volume_state, desired_state
            );
            if volume_state == desired_state {
                return Ok(());
            }
        }
    }

    pub fn wait_states(bsus: &[Bsu], desired_state: &str) -> Result<(), Box<dyn Error>> {
        let bsu_ids: Vec<String> = bsus.iter().map(|bsu| bsu.id.clone()).collect();
        debug!("fetching multiple BSU states {:?}", &bsu_ids);
        let mut request = ReadVolumesRequest::new();
        let filter = FiltersVolume {
            volume_ids: Some(bsu_ids),
            ..Default::default()
        };
        request.filters = Some(Box::new(filter));
        loop {
            api_limiter()?;
            let response = read_volumes(&*CLOUD_CONFIG.read()?, Some(request.clone()));
            if response.is_err() {
                error!("read volume response: {:?}", response);
                continue;
            }
            let volumes = response?.volumes.unwrap_or_default();
            if !volumes
                .iter()
                .filter_map(|volume| volume.state.clone())
                .any(|state| state != desired_state)
            {
                return Ok(());
            }
        }
    }

    pub fn get_state(bsu_id: &String) -> Result<String, Box<dyn Error>> {
        debug!("fetching BSU {} state", bsu_id);
        api_limiter()?;
        let mut request = ReadVolumesRequest::new();
        let filter = FiltersVolume {
            volume_ids: Some(vec![bsu_id.clone()]),
            ..Default::default()
        };
        request.filters = Some(Box::new(filter));
        let response = read_volumes(&*CLOUD_CONFIG.read()?, Some(request));
        if response.is_err() {
            error!("read volume response: {:?}", response);
        }
        let response = response?;
        let volumes = response.volumes.unwrap_or_default();
        let Some(volume) = volumes.into_iter().next() else {
            return Err(Box::new(format_err!("cannot find BSU {}", bsu_id)));
        };
        let Some(state) = volume.state else {
            return Err(Box::new(format_err!("cannot find state in BSU {}", bsu_id)));
        };
        Ok(state)
    }

    fn find_next_available_device() -> Option<String> {
        for c1 in b'b'..=b'z' {
            let device = format!("/dev/xvd{}", c1 as char);
            let path = PathBuf::from(device.clone());
            if !path.exists() {
                return Some(device);
            }
        }
        for c1 in b'b'..=b'z' {
            for c2 in b'a'..=b'z' {
                let device = format!("/dev/xvd{}{}", c1 as char, c2 as char);
                let path = PathBuf::from(device.clone());
                if !path.exists() {
                    return Some(device);
                }
            }
        }
        None
    }

    pub fn create_gib(
        drive_name: &String,
        disk_type: &DiskType,
        disk_iops_per_gib: Option<usize>,
        disk_size_gib: usize,
    ) -> Result<(), Box<dyn Error>> {
        debug!(
            "\"{}\" drive: creating BSU of type {}, size {} GiB",
            drive_name,
            disk_type.to_string(),
            disk_size_gib
        );
        api_limiter()?;
        let mut creation_request = CreateVolumeRequest::new(SUBREGION.read()?.clone());
        creation_request.volume_type = Some(disk_type.to_string());
        creation_request.iops = match disk_type {
            DiskType::Io1 => match disk_iops_per_gib {
                Some(disk_iops_per_gib) => {
                    Some((disk_size_gib * disk_iops_per_gib).max(MAX_IOPS_PER_VOLUMES) as i32)
                }
                None => {
                    Some((DEFAULT_IO1_IOPS_PER_GB * disk_size_gib).max(MAX_IOPS_PER_VOLUMES) as i32)
                }
            },
            _ => None,
        };
        creation_request.size = Some(disk_size_gib as i32);
        let create_result = match create_volume(&*CLOUD_CONFIG.read()?, Some(creation_request)) {
            Ok(create) => create,
            Err(err) => {
                debug!("\"{}\" drive: during bsu creation: {:?}", drive_name, err);
                return Err(Box::new(err));
            }
        };
        let Some(bsu) = create_result.volume else {
            return Err(Box::new(format_err!(
                "volume creation did not provide a volume object"
            )));
        };
        let Some(bsu_id) = bsu.volume_id else {
            return Err(Box::new(format_err!(
                "volume creation did provide a volume object but not volume id"
            )));
        };
        debug!("\"{}\" drive: created BSU id {}", drive_name, bsu_id);
        debug!("\"{}\" drive: adding tag to BSU {}", drive_name, bsu_id);
        api_limiter()?;
        let tag = ResourceTag::new(BSU_TAG_KEY.to_string(), drive_name.clone());
        let tag_request = CreateTagsRequest::new(vec![bsu_id.clone()], vec![tag]);
        if let Err(err) = create_tags(&*CLOUD_CONFIG.read()?, Some(tag_request)) {
            debug!(
                "\"{}\" drive: during bsu tag creation: {:?}",
                drive_name, err
            );
            return Err(Box::new(err));
        }
        Bsu::wait_state(&bsu_id, "available")?;
        Ok(())
    }
}

pub fn api_limiter() -> Result<(), Box<dyn Error>> {
    let mut limiter = API_LIMITER.lock()?;
    let waited_time_s = Instant::now().seconds() - limiter.seconds();
    let time_left = (API_LIMITER_S as i64 - waited_time_s).max(0) as u64;

    if time_left > 0 {
        debug!("api limiter sleeps for {} seconds", time_left);
        sleep(time::Duration::from_secs(time_left));
    }

    *limiter = Instant::now();
    Ok(())
}
