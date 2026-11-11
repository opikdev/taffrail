# Install Taffrail

Download Taffrail from the [Releases page](https://github.com/opikdev/taffrail/releases). Taffrail isn't code signed yet, so macOS and Windows warn you the first time you open it. The steps below get past that warning, and you only need them once.

## What you need

| System | Version | Graphics |
|---|---|---|
| macOS | 11 or later, on Apple silicon or Intel | Metal, which every Mac that runs macOS 11 has |
| Windows | 10 or 11, 64-bit | Direct3D 11, at feature level 10.1 or higher |
| Linux | 64-bit x86, with X11 or Wayland, and glibc 2.35 or later, as in Ubuntu 22.04 or Debian 12 | Vulkan, or OpenGL if Vulkan isn't available |

Taffrail draws with your graphics card, and won't start without one of these. In a Linux virtual machine, Mesa's software drivers work.

You also need a kubeconfig. Taffrail reads the one named by `KUBECONFIG`, or `~/.kube/config`.

## macOS

Download `Taffrail_<VERSION>_universal.dmg`. It runs on Apple silicon and Intel Macs.

1. Open the `.dmg` and drag Taffrail to Applications.
2. Open Taffrail. macOS shows "Taffrail" Not Opened and says Apple could not verify it. Click **Done**, not **Move to Trash**.
3. Open **System Settings**, go to **Privacy & Security**, and scroll down to **Security**.
4. Next to the message about Taffrail, click **Open Anyway**. Click **Open Anyway** again to confirm, then enter your password.

On macOS 14 or earlier, you can instead Control-click Taffrail in Applications, choose **Open**, then click **Open** again.

If macOS says Taffrail is damaged, the download is broken. Download it again, and if that doesn't help, [open an issue](https://github.com/opikdev/taffrail/issues).

## Windows

Download `taffrail_<VERSION>_x64-setup.exe`, or `taffrail_<VERSION>_x64_en-US.msi` if your company installs apps that way. The `-setup.exe` installs for your user only, into `%LOCALAPPDATA%\Taffrail`, and doesn't need an administrator. The `.msi` installs for every user, into `C:\Program Files\Taffrail`, and needs one. Both add Taffrail to the Start menu.

1. Open the installer. If Windows says it protected your PC, click **More info**, then **Run anyway**.
2. Follow the installer.

## Linux

| Your system | Download | Install |
|---|---|---|
| Ubuntu, Debian | `taffrail_<VERSION>_amd64.deb` | `sudo apt install ./taffrail_<VERSION>_amd64.deb` |
| Any other | `taffrail_<VERSION>_x86_64.AppImage` | `chmod +x taffrail_<VERSION>_x86_64.AppImage`, then open it |
| Any other, without AppImage | `taffrail_<VERSION>_x86_64.tar.gz` | `tar -xzf taffrail_<VERSION>_x86_64.tar.gz`, then run `taffrail` in the `taffrail_<VERSION>_x86_64` folder it makes |

The `.deb` adds Taffrail to your app launcher. If the AppImage doesn't open, install FUSE, which is the `fuse3` package on most systems, or run it with `--appimage-extract-and-run`.

## Opening Taffrail from the Dock or a launcher

A terminal gives the apps it starts your shell's `PATH` and `KUBECONFIG`. The Dock, Finder and Linux desktop launchers don't, so login plugins such as `aws` or `gke-gcloud-auth-plugin` wouldn't be found. On macOS and Linux, Taffrail started that way reads those variables from your login shell once at start-up. The [privacy policy](privacy.md#what-taffrail-runs) lists exactly what it keeps.

If a context still says it can't find a login plugin, set `KUBECONFIG` and `PATH` in your shell's login file, such as `~/.zprofile` or `~/.profile`, then restart Taffrail.

## Uninstall

| Your system | Uninstall |
|---|---|
| macOS | Drag Taffrail from Applications to the Trash |
| Windows | Open **Settings → Apps → Installed apps**, or **Apps & features** on Windows 10, find Taffrail, and choose **Uninstall** |
| Ubuntu, Debian | `sudo apt remove taffrail` |
| AppImage or `.tar.gz` | Delete the file or folder |

Uninstalling leaves the window state file behind. To remove it too, see [Remove everything](privacy.md#remove-everything). Taffrail never changes your kubeconfig, so there is nothing to undo there.

## Why the warnings

macOS and Windows trust an app when its developer pays for a certificate that proves who made it. Taffrail doesn't have those certificates yet. Each release is built from this repository by the [Release workflow](../.github/workflows/release.yml), so you can check exactly what went into it. See the [code signing policy](code-signing.md) for the plans.
