# Changelog

## 0.2.0

- Turn the Cloud Drives switch into persistent global power control, including
  native service, rclone, mounts, watchers and bridge reconnection.
- Preserve account, mount and sync preferences while off; prevent shutdown with
  pending streaming uploads or mount transitions.
- Keep white bar indicators for off, healthy, activity, pause, offline and errors.
- Use the public namespaced plugin ID sabb4th.cloudkit.
- Support manual setup directly in a marketplace Git checkout (--in-place).
- Add native power lifecycle coverage, installer checks, CI and marketplace docs.
