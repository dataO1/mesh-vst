# Vendored crates

- `iced_baseview` 0.6.0 (MIT / Apache-2.0). Patched to forward baseview's file drag and drop to
  iced as `window::Event::FileHovered`, `FileDropped` and `FilesHoveredLeft`, with the cursor
  position, and to accept the drop. The changes are marked `mesh vst patch`. The patch could be
  offered upstream.
