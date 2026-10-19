# Changelog

## Unreleased

- Keep the bar cloud at full theme opacity in every state, including pause,
  offline and power-off; remove its hover tooltip.
- Pipe clipboard text to wl-copy over stdin so private logs, paths and copied
  credentials never appear in shell or clipboard process arguments.

## 0.3.0

- Follow the live Omarchy bar palette instead of forcing a white cloud icon.
- Organize settings into six sections with direct local/cloud folder mappings,
  existing folder controls, per-account streaming and optional login service.
- Expose cache age and free-space reserve, preserving zero-valued limits.
- Add validated transfer/checker counts and concurrent folder limits, applied
  to future sync jobs without cancelling active transfers.
- Add QML settings interaction/theme coverage and real-rclone settings tests.
- Keep private filenames and error details out of notification process arguments.
- Preserve VFS lookup for other mounted accounts when removing an account, and
  send the supported string parameter when refreshing the streaming cache.

## 0.2.0

- Turn the Cloud Drives switch into persistent global power control, including
  native service, rclone, mounts, watchers and bridge reconnection.
- Preserve account, mount and sync preferences while off; prevent shutdown with
  pending streaming uploads or mount transitions.
- Keep white bar indicators for off, healthy, activity, pause, offline and errors.
- Use the public namespaced plugin ID sabb4th.cloudkit.
- Support manual setup directly in a marketplace Git checkout (--in-place).
- Add native power lifecycle coverage, installer checks, CI and marketplace docs.
