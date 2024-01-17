# Run Dependencies

BSUd is only for Linux-based OS on Outscale's cloud for now.

BSUd will need to run those external commands:
- lvm
- btrfs
- [Outscale's API Access Key and Secret Key](https://docs.outscale.com/en/userguide/About-Access-Keys.html)

# Configuration

## config.json

BSUd is configured through a json configuration file.
The following section describe a simple configuration (see [config.json](config.json)):

- `authentification`
  - `access-key`: optional if OSC_ACCESS_KEY env var is set.
  - `secret-key`: optional if OSC_SECRET_KEY env var is set.
- `drives`
  - `name`: unique drive's name. Be sure to use an unique name across your Outscale account otherwise, BSUd cannot differentiate drives and will try to attach them.
  - `target`: between "online" (default), "offline" and "delete".
  - `mount-path`: absolute path where BSUd will mount the scaled file system.
  - `disk-iops-per-gib`: BSU iops to allocate per GibiBytes (for io1 disks).
  - `max-total-size-gib`: Limit the maximal size a drive can offer.
  - `disk-scale-factor-perc`: Controls the size of the next BSU to be created regarding the size of the largest or smallest existing BSU in the drive.
  - `min-used-space-perc` controls when to scale down (remove a BSU) accordingly to the used percentage in the drive.
  - `max-bsu-count`: maximal allowed number of BSU in the drive.

## Environment variables

- OSC_ACCESS_KEY
- OSC_SECRET_KEY

# Usage

- Get version: `bsud --version`
- Manually run bsud: `bsud -c docs/config.json`

`bsud` will look for `/etc/osc/bsud.json` configuration file path by default.

# Creating or updating a drive

Just add or edit drive in BSUd configuration and restart daemon.
Note that changing drive name is not supported for now and will just create a new fresh drive.

# About drive targets

When drive target is configured to "online" (default), all BSU are attached and the drive is maintained available to user.

Drives which have a "offline" target are unmounted and all its BSUs are detached from the VM.

Drives which have a "delete" target are umounted, all its BSU are detached from the VM and all its BSU are deleted.

## About auto-scaling

BSUd is using exponential auto-scaling. It will create an exponentially larger BSU to expand the drive while limiting the number of attached BSU. The size of created BSU will be adjusted depending of a scaling factor (see `disk-scale-factor-perc`).

Example: If the largest BSU in the drive is 20Gib and the scale factor is set to 10%, then the next BSU to be added to the drive will be will be 20 * 1.1 = 22Gib.

BSUd may also create BSU which are 10% smaller in some particular cases in order to balance drive size repartition. More details on how this can occur in [dev documentation](develop.md).

A minimal and maximal space usage can be defined in order to trigger scaling (see `min-used-space-perc` and `max-used-space-perc`).

Example: If the drive is 89% full and the `max-used-space-perc` is set to 85%, then the drive will scale up (adding a BSU).
Example: If the drive is 19% full and the `min-used-space-perc` is set to 20%, then the drive will scale down (remove a BSU).

VMs cannot attach an infinite number of disks. `max-bsu-count` will limit the number of attached BSU without limiting drive's maximal size. BSUd will scale up and migrate any data before removing a BSU.
BSUd will maintain `max-bsu-count` minus 1 in order to be able to add one more disk to scale up. Once `max-bsu-count` BSU reached, BSUd will try to remove the smallest disk.
