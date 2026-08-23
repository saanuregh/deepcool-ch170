# DeepCool CH170 Display Controller

Drives the DeepCool CH170 Digital display with live CPU/GPU stats on Windows,
reading sensors directly from the hardware. It cycles through CPU-frequency, GPU,
and CPU-fan views.

> **Hardware-specific.** The CPU and fan register maps are written for an
> **AMD Zen CPU** + **Nuvoton NCT6701D** SuperIO chip + **NVIDIA GPU**. Other
> hardware needs the maps in [`src/sensors/`](src/sensors/) adapted (the GPU path
> is portable across NVIDIA cards).

## Requirements

- Windows, with the target hardware above.
- [PawnIO](https://pawnio.eu) installed (`winget install namazso.PawnIO`) — the
  signed kernel driver used for low-level CPU/fan access.
- **Administrator.** PawnIO only grants access to elevated processes; the app
  embeds a manifest, so Windows prompts for UAC on launch.
- NVIDIA driver (provides `nvml.dll`); if absent, GPU values read as zero.

## Build & install

Uses [mise](https://mise.jdx.dev):

```bash
mise install        # toolchain
mise run build      # -> target\release\deepcool-ch170.exe
mise run install    # build, copy to C:\bin, register a logon autostart task (elevates)
mise run uninstall  # remove the autostart task
```

`mise tasks` lists the rest (`dev`, `test`, `check`).

## Run

Launch the executable and accept the UAC prompt. Subcommands (also run elevated):

```bash
deepcool-ch170.exe --install | --uninstall | --status  # logon autostart task
deepcool-ch170.exe --dump-sensors                       # print readings, no display
```

Autostart uses a Task Scheduler "at logon" task with highest privileges, so it
starts silently at logon — the Startup folder can't launch an elevated app cleanly.

On shutdown, logoff or Ctrl+C the display loop stops and closes the HID device and
the PawnIO handles before the OS tears the session down, so Windows never has to
force-kill it. A hidden top-level window answers `WM_QUERYENDSESSION` immediately —
without it the process has neither a window nor a console to be asked through, and
gets terminated mid-USB-write.

## How it reads sensors

| Metric | Source |
| --- | --- |
| CPU temp / package power / effective clock, fan RPM | [PawnIO](https://pawnio.eu) (MSR + SMN reads, SuperIO port I/O) |
| CPU load | Win32 `GetSystemTimes` |
| GPU temp / power / usage / clock | NVIDIA NVML |

The register/decoding logic mirrors
[LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor).
The signed PawnIO modules are vendored under
[`resources/pawnio/`](resources/pawnio/) and embedded at build time.

## License

MIT — see [LICENSE](LICENSE). Vendored PawnIO modules are LGPL-2.1
(`resources/pawnio/COPYING`), from
[namazso/PawnIO.Modules](https://github.com/namazso/PawnIO.Modules).

## Acknowledgments

- [Nortank12](https://github.com/Nortank12) — original Linux implementation
- [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) — sensor register logic
- [namazso](https://github.com/namazso) — [PawnIO](https://github.com/namazso/PawnIO) + modules
