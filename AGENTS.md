# Project constraints

- `eli` must support Windows, Linux, and macOS. New functionality and dependencies must preserve compatibility with all three platforms unless a requirement explicitly states otherwise.
- Platform-specific implementations must be isolated with conditional compilation and expose a shared cross-platform interface from `eli-lib`.
- Boot-disk discovery must enumerate mounted, readable partitions on the current operating system; it must not assume that partitions are represented by Windows drive letters.
