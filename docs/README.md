```
🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥
🔥     WORK IN PROGRESS      🔥
🔥 DO NOT USE IN PRODUCTION  🔥
🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥
```

# BSUd

BSUd creates a virtual drive on your linux machine on [Outscale's cloud](https://outscale.com/). The drive is composed of aggregated cloud block devices ([Block Storage Units](https://docs.outscale.com/en/userguide/Block-Storage-Unit-BSU.html)).
BSUd will dynamically add and remove BSUs accordingly to real file occupation on the drive while limiting the number of attached BSUs.
Scaling up and down is done without putting the drive offline.

BSUd directly run system commands such as [LVM](https://en.wikipedia.org/wiki/Logical_Volume_Manager_(Linux)) to manage block aggregation. All commands are logged and administrators can easily inspect the system with or without BSUd running. No dark magic.

Here is an example of what a BSUd drive could look like under LVM perspective:
```bash
$ sudo pvdisplay -S vgname=example -C --separator '  |  ' -o pv_name,vg_name,pv_size,vg_size;
         PV  |       VG  |     PSize  |  VSize
  /dev/xvdb  |  example  |  <70.00g   |  1.44t
  /dev/xvdc  |  example  |  <101.00g  |  1.44t
  /dev/xvdd  |  example  |  <84.00g   |  1.44t
  /dev/xvde  |  example  |  <122.00g  |  1.44t
  /dev/xvdg  |  example  |  <147.00g  |  1.44t
  /dev/xvdh  |  example  |  <177.00g  |  1.44t
  /dev/xvdi  |  example  |  <213.00g  |  1.44t
  /dev/xvdj  |  example  |  <256.00g  |  1.44t
  /dev/xvdk  |  example  |  <308.00g  |  1.44t
```

- 🎁 [Install](install.md)
- 🚀 [Use](use.md)
- 🔧 [Develop](develop.md)
- 💡 [Contribute](CONTRIBUTING.md)

# License

BSUd is licensed under BSD-3-Clause. See [LICENSE](../LICENSE) for more details.