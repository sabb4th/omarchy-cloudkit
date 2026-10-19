### Repository URL

https://github.com/sabb4th/omarchy-cloudkit

### Category

Productivity

### Tags

bar, quickshell, system

### Suggest a missing tag

_No response_

### Maintainer notes

omarchy-cloudkit 0.2.0 (plugin ID: sabb4th.cloudkit) is a Rust rewrite of Clouds
Guacamole with retained MIT notices and attribution. It integrates rclone cloud
streaming, FUSE mounts, bidirectional offline folder sync, and a QML bar panel.
The global power switch stops background processes and persists across sessions.

Manual setup is required: after adding the repository without --enable, run
./install.sh --in-place from ~/.config/omarchy/plugins/sabb4th.cloudkit.
Please apply the manual-setup label. The installer compiles Rust locally and can
install missing Arch packages using pacman via sudo/pkexec. Runtime dependencies,
file/network access, update and removal instructions are documented in README.md
and docs/MARKETPLACE.md. This is an early development release; it does not offer
upload-only backup or claim security certification. No preview is submitted.

### Submission checklist

- [ ] The repository is public and contains installation and removal instructions.
- [ ] I have documented the plugin license and any external dependencies.
- [ ] I confirm that I own or have permission to submit this plugin and its preview assets.
- [ ] The plugin does not overwrite user configuration without explicit consent.
- [ ] I understand that approval is for listing and is not a security review.
