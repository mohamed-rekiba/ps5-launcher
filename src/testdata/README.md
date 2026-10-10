# Test data

Real output of system tools, captured in PS5 Launcher OS's spike VM boot (Fedora 44, see
`packaging/os/boottest/report` on the spike branch). The parsers' tests read these files, so they
check against what the tools really print. Do not edit them by hand; capture them again instead.

| File | Command |
|---|---|
| `fedora44-vm-lsblk.json` | `lsblk --json -b -o NAME,PATH,SIZE,TYPE,FSTYPE,MOUNTPOINT,RM,HOTPLUG,LABEL,MODEL` |
| `fedora44-vm-bootc-status.txt` | `bootc status --json`, as printed to a terminal: a progress line comes before the JSON |
| `fedora44-vm-wpctl-status.txt` | `wpctl status` in the user's session (colours removed) |
