# Marketplace preparation

Plugin: **omarchy-cloudkit**, version **0.3.0**, ID **sabb4th.cloudkit**.
Repository: https://github.com/sabb4th/omarchy-cloudkit
Category: **Productivity**. Tags: **bar, quickshell, system**.

## Installation and update

This plugin requires **manual setup**. Omarchy's manager only clones files; it
neither runs install hooks nor builds Rust. Maintainers should apply the
`manual-setup` label before approving a listing.

```bash
omarchy plugin add https://github.com/sabb4th/omarchy-cloudkit.git
cd "$HOME/.config/omarchy/plugins/sabb4th.cloudkit"
./install.sh --in-place
```

The installer documents and requests elevation only for missing Arch packages.
Rust builds locally with Cargo.lock; rclone 1.75.2 or newer is required. It does not download or execute a release
binary. After updating the checkout through the manager, repeat the installer.
The installer preserves the manager's .git directory. See README for removal.

## Capabilities and files

- QML runs in the shared Omarchy shell; Rust runs as the desktop user.
- rclone accesses user-configured cloud providers and may invoke a browser for
  OAuth. No telemetry or analytics endpoint is configured by CloudKit.
- Account credentials remain in rclone.conf / RCLONE_CONFIG. The plugin's state,
  settings, logs, filters and local backups use private XDG app directories;
  existing Guacamole profiles are reused for compatibility.
- Streaming uses FUSE and a local rclone cache. Sync uses bisync and may update
  or delete files on either side. The plugin does not offer upload-only backup.
- Optional systemd user service; installer adds a CLI symlink and enables the
  widget. Missing packages use sudo or pkexec with pacman. No sudoers edits.
- Global power-off persists across sessions and terminates the native service,
  its rclone children and file watchers. The bar control remains available.
- No account credentials, personal configuration or generated binary belongs in
  the repository. Python files are upstream reference and test tooling only.

## Submission

The completed draft is [MARKETPLACE-SUBMISSION.md](MARKETPLACE-SUBMISSION.md).
The owner must review the title and body, including the five checklist statements,
before submitting it. No listing approval or security certification is claimed.

After owner approval:

```bash
gh issue create --repo omacom/omarchy-plugin-marketplace \
  --title '[Plugin]: omarchy-cloudkit' \
  --body-file docs/MARKETPLACE-SUBMISSION.md
```

Official references:
- https://plugins.omarchy.org/publish.html
- https://github.com/omacom/omarchy-plugin-marketplace/blob/main/SUBMISSION.md
- https://github.com/omacom/omarchy-plugin-marketplace/blob/main/SECURITY.md
