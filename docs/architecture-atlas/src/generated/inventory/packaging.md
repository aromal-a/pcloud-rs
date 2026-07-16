# Packaging and service files

This generated page covers **111** Git-visible files.

Kind summary: script: 27, documentation: 21, file: 21, packaging/service: 11, YAML/config: 5, example: 4, in: 3, configuration: 2, xml: 2, rb: 2, 1: 2, asset: 2, pcloudd: 1, logrotate: 1, container build: 1, rc: 1, 5: 1, txt: 1, fc: 1, te: 1, rtf: 1

| File | Kind | Source-derived role |
|---|---|---|
| [`packaging/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/README.md) | documentation | `packaging/` — In-tree packaging assets |
| [`packaging/apparmor/usr.local.bin.pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/apparmor/usr.local.bin.pcloudd) | pcloudd | include &lt;tunables/global |
| [`packaging/appimage/AppRun`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/appimage/AppRun) | file | PLATFORM: Linux |
| [`packaging/appimage/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/appimage/README.md) | documentation | pcloud-rs AppImage |
| [`packaging/appimage/build-appimage.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/appimage/build-appimage.sh) | script | PLATFORM: Linux |
| [`packaging/appimage/pcloud-rs.desktop`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/appimage/pcloud-rs.desktop) | packaging/service | PLATFORM: Linux |
| [`packaging/bsd/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/bsd/README.md) | documentation | BSD packaging and service assets |
| [`packaging/chocolatey/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/chocolatey/README.md) | documentation | Chocolatey packaging |
| [`packaging/chocolatey/pcloud-rs.nuspec`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/chocolatey/pcloud-rs.nuspec) | packaging/service | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/chocolatey/tools/chocolateyinstall.ps1`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/chocolatey/tools/chocolateyinstall.ps1) | script | PLATFORM: Windows 10/11 (x64) |
| [`packaging/chocolatey/tools/chocolateyuninstall.ps1`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/chocolatey/tools/chocolateyuninstall.ps1) | script | PLATFORM: Windows 10/11 (x64) |
| [`packaging/debian/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/README.md) | documentation | Debian / nfpm packaging |
| [`packaging/debian/cargo-deb.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/cargo-deb.toml) | configuration | Optional cargo-deb metadata snippet. |
| [`packaging/debian/control`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/control) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/debian/nfpm.yaml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/nfpm.yaml) | YAML/config | nfpm configuration for pcloud-rs Debian/RPM packaging. |
| [`packaging/debian/pcloud-rs.logrotate`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/pcloud-rs.logrotate) | logrotate | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/debian/postinst`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/postinst) | file | postinst script for pcloud-rs |
| [`packaging/debian/postrm`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/debian/postrm) | file | postrm script for pcloud-rs |
| [`packaging/docker/Dockerfile`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/docker/Dockerfile) | container build | syntax=docker/dockerfile:1.7 |
| [`packaging/docker/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/docker/README.md) | documentation | pcloud-rs Docker image |
| [`packaging/docker/docker-compose.yml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/docker/docker-compose.yml) | YAML/config | Example compose file for the pcloud-rs Rust daemon. |
| [`packaging/docker/entrypoint.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/docker/entrypoint.sh) | script | PLATFORM: Linux (container runtime) |
| [`packaging/dragonfly/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/dragonfly/README.md) | documentation | DragonFly BSD service |
| [`packaging/dragonfly/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/dragonfly/pcloudd) | file | PLATFORM: DragonFly BSD 6.4+ |
| [`packaging/flatpak/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/flatpak/README.md) | documentation | pcloud-rs Flatpak packaging |
| [`packaging/flatpak/com.pcloud.pcloud-rs.desktop`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/flatpak/com.pcloud.pcloud-rs.desktop) | packaging/service | PLATFORM: Linux (Flatpak runtime) |
| [`packaging/flatpak/com.pcloud.pcloud-rs.metainfo.xml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/flatpak/com.pcloud.pcloud-rs.metainfo.xml) | xml | PLATFORM: Linux (Flatpak runtime) |
| [`packaging/flatpak/com.pcloud.pcloud-rs.yaml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/flatpak/com.pcloud.pcloud-rs.yaml) | YAML/config | PLATFORM: Linux (Flatpak runtime) |
| [`packaging/freebsd/pcloudd.rc`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/freebsd/pcloudd.rc) | rc | PLATFORM: FreeBSD 13+ |
| [`packaging/homebrew/Casks/fuse-t.rb`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/homebrew/Casks/fuse-t.rb) | rb | PLATFORM: macOS (Homebrew Cask) |
| [`packaging/homebrew/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/homebrew/README.md) | documentation | Homebrew packaging |
| [`packaging/homebrew/pcloud-rs.rb`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/homebrew/pcloud-rs.rb) | rb | PLATFORM: macOS |
| [`packaging/init/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/README.md) | documentation | Cross-Init Service Scripts |
| [`packaging/init/common/pcloudd-wrapper.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/common/pcloudd-wrapper.sh) | script | Platforms whose service manager starts the wrapper after dropping |
| [`packaging/init/common/pcloudd.env.example`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/common/pcloudd.env.example) | example | pcloud-rs daemon service environment |
| [`packaging/init/dinit/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/dinit/pcloudd) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/freebsd/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/freebsd/pcloudd) | file | PROVIDE: pcloudd |
| [`packaging/init/netbsd/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/netbsd/pcloudd) | file | PROVIDE: pcloudd |
| [`packaging/init/openbsd/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/openbsd/pcloudd) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/openrc/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/openrc/pcloudd) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/runit/pcloudd/log/run`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/runit/pcloudd/log/run) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/runit/pcloudd/run`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/runit/pcloudd/run) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/s6/pcloudd/finish`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/s6/pcloudd/finish) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/s6/pcloudd/run`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/s6/pcloudd/run) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/init/sysvinit/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/init/sysvinit/pcloudd) | file | BEGIN INIT INFO |
| [`packaging/macos/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/README.md) | documentation | macOS packaging |
| [`packaging/macos/build-dmg.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/build-dmg.sh) | script | Build a macOS .dmg disk image for pcloud-rs. |
| [`packaging/macos/build-pkg.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/build-pkg.sh) | script | Build a macOS .pkg installer for pcloud-rs. |
| [`packaging/macos/com.pcloud.pcloud-rs.plist`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/com.pcloud.pcloud-rs.plist) | packaging/service | Avoid rapid restart loops: wait 10 seconds before restarting on crash. |
| [`packaging/macos/com.pcloud.pcloudd.plist`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/com.pcloud.pcloudd.plist) | packaging/service | argv\[0\] is the daemon binary; `serve` selects the long-running IPC |
| [`packaging/macos/configure-user.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/configure-user.sh) | script | Materialize and load the packaged per-user LaunchAgent. |
| [`packaging/macos/entitlements.plist`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/entitlements.plist) | packaging/service | Required for outbound HTTPS to the pCloud API. |
| [`packaging/macos/first-run.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/first-run.sh) | script | Interactive first-run setup for pcloud-rs on macOS. |
| [`packaging/macos/install.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/install.sh) | script | macOS install script for pcloud-rs. |
| [`packaging/macos/launchd-status.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/launchd-status.sh) | script | Show the status of the pcloud-rs LaunchAgent and daemon. |
| [`packaging/macos/setup-keychain.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/setup-keychain.sh) | script | Set up pCloud credentials in the macOS Keychain for auto-login. |
| [`packaging/macos/uninstall.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/macos/uninstall.sh) | script | Uninstall pcloud-rs from macOS. |
| [`packaging/man/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/man/README.md) | documentation | pcloud-rs manpages |
| [`packaging/man/pcloud.conf.5`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/man/pcloud.conf.5) | 5 | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/man/pcloudc.1`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/man/pcloudc.1) | 1 | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/man/pcloudd.1`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/man/pcloudd.1) | 1 | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/README.md) | documentation | NAS packages (tier 2) |
| [`packaging/nas/asustor/build-apk.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/build-apk.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/asustor/config.json.in`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/config.json.in) | in | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/asustor/description.txt`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/description.txt) | txt | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/asustor/icon.png`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/icon.png) | asset | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/asustor/icon.svg`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/icon.svg) | asset | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/asustor/start-stop.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/asustor/start-stop.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/common/pcloudd-supervisor.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/common/pcloudd-supervisor.sh) | script | Shared NAS process supervisor. Vendor lifecycle scripts provide the paths. |
| [`packaging/nas/common/test-supervisor.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/common/test-supervisor.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/qnap/build-qpkg.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/qnap/build-qpkg.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/qnap/pcloud-rs.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/qnap/pcloud-rs.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/qnap/qpkg.cfg.in`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/qnap/qpkg.cfg.in) | in | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/synology/build-spk.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/synology/build-spk.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/synology/conf/privilege`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/synology/conf/privilege) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/synology/scripts/postinst`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/synology/scripts/postinst) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/synology/scripts/start-stop-status`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/synology/scripts/start-stop-status) | file | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/nas/validate.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/nas/validate.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/netbsd/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/netbsd/pcloudd) | file | PLATFORM: NetBSD 10+ |
| [`packaging/openbsd/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/openbsd/pcloudd) | file | PLATFORM: OpenBSD 7.x |
| [`packaging/scoop/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/scoop/README.md) | documentation | pcloud-rs Scoop packaging |
| [`packaging/scoop/pcloud-rs.json`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/scoop/pcloud-rs.json) | configuration | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/scripts/verify-reproducibility.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/scripts/verify-reproducibility.sh) | script | verify-reproducibility.sh |
| [`packaging/selinux/pcloud-rs.fc`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/selinux/pcloud-rs.fc) | fc | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/selinux/pcloud-rs.te`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/selinux/pcloud-rs.te) | te | SELinux policy for the pcloud-rs Rust daemon. |
| [`packaging/signing/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/signing/README.md) | documentation | Signing &amp; Notarisation Pipeline — Operator Guide |
| [`packaging/signing/notarize-macos.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/signing/notarize-macos.sh) | script | PLATFORM: macOS only. |
| [`packaging/signing/sign-macos.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/signing/sign-macos.sh) | script | PLATFORM: macOS only. |
| [`packaging/signing/sign-windows.ps1`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/signing/sign-windows.ps1) | script | PLATFORM: Windows only. |
| [`packaging/snap/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/snap/README.md) | documentation | pcloud-rs Snap packaging |
| [`packaging/snap/snapcraft.yaml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/snap/snapcraft.yaml) | YAML/config | PLATFORM: Linux (Snap / snapd) |
| [`packaging/solarish/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/solarish/README.md) | documentation | illumos and Oracle Solaris service |
| [`packaging/solarish/pcloudd`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/solarish/pcloudd) | file | SMF start method for illumos and Oracle Solaris. |
| [`packaging/solarish/pcloudd.xml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/solarish/pcloudd.xml) | xml | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/systemd/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/README.md) | documentation | pcloudd systemd packaging |
| [`packaging/systemd/override-fuse.conf.example`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/override-fuse.conf.example) | example | pcloudd.service drop-in override — enable FUSE-mounted pCloud drive |
| [`packaging/systemd/override-user.conf.example`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/override-user.conf.example) | example | pcloudd.service drop-in override — legacy compatibility for --user deployments |
| [`packaging/systemd/override.conf.example`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/override.conf.example) | example | pcloudd.service drop-in override — OPT-IN strict egress allow-listing |
| [`packaging/systemd/pcloudd-user.service`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/pcloudd-user.service) | packaging/service | User-scoped companion to pcloudd.service. This unit deliberately avoids |
| [`packaging/systemd/pcloudd.service`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/pcloudd.service) | packaging/service | This unit is system-scoped only: |
| [`packaging/systemd/pcloudd.socket`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/pcloudd.socket) | packaging/service | Owner-only local IPC. This unit is kept as a future socket-activation |
| [`packaging/unix/INSTALL.md.in`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/unix/INSTALL.md.in) | in | pcloud-rs @VERSION@ for @PLATFORM@/@ARCH@ |
| [`packaging/unix/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/unix/README.md) | documentation | Portable Unix package candidates |
| [`packaging/unix/build-tarball.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/unix/build-tarball.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/unix/validate.sh`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/unix/validate.sh) | script | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/windows/wix/License.rtf`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/windows/wix/License.rtf) | rtf | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/windows/wix/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/windows/wix/README.md) | documentation | pcloud-rs WiX MSI |
| [`packaging/windows/wix/pcloud-rs-bundle.wxs`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/windows/wix/pcloud-rs-bundle.wxs) | packaging/service | Packaging, service lifecycle, installer, or platform-distribution asset. |
| [`packaging/windows/wix/pcloud-rs.wxs`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/windows/wix/pcloud-rs.wxs) | packaging/service | Icon: not yet shipped. Uncomment once pcloud-rs.ico is committed. |
| [`packaging/winget/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/winget/README.md) | documentation | winget packaging |
| [`packaging/winget/pcloud-rs.yaml`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/winget/pcloud-rs.yaml) | YAML/config | PLATFORM: Windows |
