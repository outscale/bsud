mod args;
mod bsu;
mod config;
mod drive;
mod fs;
mod lvm;
mod utils;

use drive::Drives;
use log::{debug, error, info, warn};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    env_logger::init();
    info!("starting bsud v{}", VERSION);

    let mut signals = Signals::new([SIGINT, SIGTERM]).expect("cannot init signals");

    let args = args::parse();
    debug!("args: {:?}", args);

    let config = config::load(args.config_path).unwrap_or_else(|err| {
        error!("cannot init configuration: {}", err);
        exit(1)
    });
    debug!("config: {:?}", config);

    if !pre_flight_check() {
        exit(1);
    }

    let mut drives = Drives::run(config).unwrap_or_else(|err| {
        error!("cannot run drives: {}", err);
        exit(1);
    });

    loop {
        for sig in signals.forever() {
            warn!("received signal {:?}", sig);
            match sig {
                SIGINT | SIGTERM => {
                    if let Err(err) = drives.stop() {
                        error!("error while stopping: {}", err);
                    }
                    exit(0);
                }
                _unmanaged_sig => {
                    error!("unmanaged signal {}", _unmanaged_sig);
                }
            }
        }
    }
}

fn pre_flight_check() -> bool {
    let mut ret = true;
    if utils::exec("lvm", &["fullreport"]).is_err() {
        error!("cannot get lvm fullreport, check installation and permissions");
        ret = false;
    }
    if utils::exec("btrfs", &["filesystem", "show"]).is_err() {
        error!("cannot get run btrfs, check installation and permissions");
        ret = false;
    }
    ret
}

fn exit(code: i32) -> ! {
    info!("exiting bsud v{} with code {}", VERSION, code);
    process::exit(code)
}
