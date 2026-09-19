// 测试专用的 fake 后端（仅 test 构建编译）。
//
// 允许编排、预检和提交时序在 Windows、Linux、macOS 上以真实文件系统
// 做确定性单元测试，不依赖注册表、Shell 或外部进程。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::archive::ArchiveEntry;
use super::{CursorSnapshot, RegistryValueSnapshot, ThemeBackend, ThemePaths};

static TEST_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 创建不经过宿主机符号链接别名的唯一测试目录。
///
/// macOS 的临时目录通常从 `/var` 返回，而 `/var` 是指向 `/private/var` 的
/// 符号链接。先规范化已经存在的临时目录根，避免路径安全测试把系统别名误判为
/// 测试数据中的逃逸链接。
pub fn test_root(label: &str) -> PathBuf {
    let temporary = std::env::temp_dir();
    let temporary = std::fs::canonicalize(&temporary).unwrap_or(temporary);
    let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = temporary.join(format!(
        "eli-theme-{label}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// 一个可被 fake 后端“打开”的归档：列出条目并按白名单在目标目录中物化文件。
#[derive(Debug, Clone)]
pub struct FakeArchive {
    pub entries: Vec<ArchiveEntry>,
    /// 相对路径（`/` 分隔）→ 文件内容。
    pub files: HashMap<String, Vec<u8>>,
}

#[derive(Debug, Default)]
pub struct FakeState {
    /// 按调用路径记录的后端事件（列表/解压/提交/刷新等）。
    pub log: Vec<String>,
    pub archives: HashMap<PathBuf, FakeArchive>,
    /// 按文件名折叠匹配的嵌套归档（EIS/EMS/ESS 解压后位于随机 staging 目录）。
    pub nested_by_name: HashMap<String, FakeArchive>,
    /// 每次解压调用收到的白名单，用于验证 ELS 等跳过资源不会被物化。
    pub extraction_whitelists: Vec<Vec<String>>,
    /// 需要模拟修改失败的 `.lnk` 绝对路径。
    pub link_modify_failures: Vec<PathBuf>,
    /// 模拟的 HKCU\Control Panel\Cursors 槽位。
    pub cursor_slots: [Option<String>; 17],
    pub cursor_slot_types: [Option<u32>; 17],
    pub cursor_default: Option<String>,
    pub cursor_default_type: Option<u32>,
    pub cursor_schemes: HashMap<String, String>,
    pub cursor_scheme_types: HashMap<String, u32>,
    pub cursor_dirs: Vec<PathBuf>,
    pub spi_calls: usize,
    pub shell_running: bool,
    pub shell_events: Vec<String>,
    pub modified_links: Vec<(PathBuf, PathBuf)>,
    pub esc_runs: Vec<PathBuf>,
    pub wall_runs: Vec<PathBuf>,
    pub temporary_permission_targets: Vec<PathBuf>,
    /// 并发保护：提交阶段后端调用期间 active 始终为 0 或 1。
    pub active: usize,
    pub max_active: usize,
    pub fail_jpeg: bool,
    pub fail_icon: bool,
    pub fail_plain_image: bool,
    pub fail_cursor_validation: bool,
    pub fail_cursor_snapshot: bool,
    pub fail_wall: bool,
    pub fail_esc: bool,
    pub fail_cursor_write: bool,
    pub fail_spi: bool,
    pub fail_notify: bool,
    pub shell_stop_fails: bool,
    pub shell_start_fails: bool,
}

pub struct FakeBackend {
    pub paths: ThemePaths,
    pub state: Mutex<FakeState>,
}

impl FakeBackend {
    pub fn new(archive_root: PathBuf) -> Self {
        let paths = ThemePaths {
            system_root: archive_root.join("Windows"),
            staging_root: archive_root.join("Users/Theme/eli/staging"),
            wallpaper_dir: archive_root.join("Users/Theme/eli/wallpaper"),
            icon_root: archive_root.join("Users/Icon"),
            cursor_root: archive_root.join("Windows/Cursors/Edgeless"),
            desktop_roots: vec![archive_root.join("Desktop")],
            icon_cache_dir: archive_root.join("Cache"),
        };
        Self {
            paths,
            state: Mutex::new(FakeState::default()),
        }
    }

    pub fn push(&self, event: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.log.push(event.to_owned());
        }
    }

    fn commit_span<T>(&self, event: &str, operation: impl FnOnce() -> T) -> T {
        self.push(event);
        if let Ok(mut state) = self.state.lock() {
            state.active += 1;
            if state.active > state.max_active {
                state.max_active = state.active;
            }
        }
        let result = operation();
        if let Ok(mut state) = self.state.lock() {
            state.active -= 1;
        }
        result
    }
}

impl ThemeBackend for FakeBackend {
    fn theme_paths(&self) -> io::Result<ThemePaths> {
        self.push("theme_paths");
        Ok(self.paths.clone())
    }

    fn require_seven_zip(&self) -> io::Result<()> {
        self.push("require_seven_zip");
        Ok(())
    }

    fn require_pecmd(&self) -> io::Result<()> {
        self.push("require_pecmd");
        Ok(())
    }

    fn verify_shell_context(&self) -> io::Result<()> {
        self.push("verify_shell_context");
        Ok(())
    }

    fn list_archive(&self, source: &Path) -> io::Result<Vec<ArchiveEntry>> {
        self.push("list_archive");
        let state = self.state.lock().unwrap();
        let archive = state
            .archives
            .get(&fs_canonical(source))
            .or_else(|| state.archives.get(source))
            .or_else(|| {
                let name = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(super::transaction::case_fold)?;
                state.nested_by_name.get(&name)
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("fake archive not registered: {}", source.display()),
                )
            })?;
        Ok(archive.entries.clone())
    }

    fn extract_archive_entries(
        &self,
        source: &Path,
        destination: &Path,
        entries: &[String],
    ) -> io::Result<()> {
        self.push("extract_archive_entries");
        let archive = {
            let state = self.state.lock().unwrap();
            state
                .archives
                .get(&fs_canonical(source))
                .or_else(|| state.archives.get(source))
                .or_else(|| {
                    let name = source
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(super::transaction::case_fold)?;
                    state.nested_by_name.get(&name)
                })
                .cloned()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("fake archive not registered: {}", source.display()),
                    )
                })?
        };
        self.state
            .lock()
            .unwrap()
            .extraction_whitelists
            .push(entries.to_vec());
        std::fs::create_dir_all(destination)?;
        for entry in &archive.entries {
            let matched = entries
                .iter()
                .any(|whitelist| super::transaction::name_matches(whitelist, &entry.path));
            if !matched {
                continue;
            }
            let materialized = path_from_archive_name(destination, &entry.path);
            if entry.is_directory {
                std::fs::create_dir_all(materialized)?;
                continue;
            }
            if let Some(parent) = materialized.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let contents = archive.files.get(&entry.path).cloned().unwrap_or_default();
            std::fs::write(materialized, contents)?;
        }
        Ok(())
    }

    fn decode_jpeg(&self, _bytes: &[u8]) -> io::Result<()> {
        self.push("decode_jpeg");
        let state = self.state.lock().unwrap();
        if state.fail_jpeg {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fake JPEG decode failure",
            ))
        } else {
            Ok(())
        }
    }

    fn decode_icon(&self, _bytes: &[u8]) -> io::Result<()> {
        self.push("decode_icon");
        let state = self.state.lock().unwrap();
        if state.fail_icon {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fake ICO decode failure",
            ))
        } else {
            Ok(())
        }
    }

    fn decode_plain_image(&self, _bytes: &[u8]) -> io::Result<()> {
        self.push("decode_plain_image");
        let state = self.state.lock().unwrap();
        if state.fail_plain_image {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "fake static image decode failure",
            ))
        } else {
            Ok(())
        }
    }

    fn validate_cursor_file(&self, path: &Path) -> io::Result<()> {
        self.push("validate_cursor_file");
        let state = self.state.lock().unwrap();
        if state.fail_cursor_validation {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("fake cursor validation failure: {}", path.display()),
            ))
        } else {
            Ok(())
        }
    }

    fn apply_wallpaper(&self, image: &Path) -> io::Result<()> {
        self.commit_span("apply_wallpaper", || ());
        let mut state = self.state.lock().unwrap();
        state.wall_runs.push(image.to_owned());
        if state.fail_wall {
            Err(io::Error::other("fake WALL command failure"))
        } else {
            Ok(())
        }
    }

    fn execute_esc(&self, script: &Path) -> io::Result<()> {
        self.commit_span("execute_esc", || ());
        let mut state = self.state.lock().unwrap();
        state.esc_runs.push(script.to_owned());
        if state.fail_esc {
            Err(io::Error::other("fake PECMD LOAD failure"))
        } else {
            Ok(())
        }
    }

    fn snapshot_cursors(&self, target_scheme: &str) -> io::Result<CursorSnapshot> {
        self.commit_span("snapshot_cursors", || ());
        let state = self.state.lock().unwrap();
        if state.fail_cursor_snapshot {
            return Err(io::Error::other("fake cursor snapshot failure"));
        }
        Ok(CursorSnapshot {
            slots: std::array::from_fn(|slot| {
                state.cursor_slots[slot].as_ref().map(|value| {
                    fake_registry_snapshot(value, state.cursor_slot_types[slot].unwrap_or(2))
                })
            }),
            default_scheme: state
                .cursor_default
                .as_ref()
                .map(|value| fake_registry_snapshot(value, state.cursor_default_type.unwrap_or(1))),
            target_scheme: state.cursor_schemes.get(target_scheme).map(|value| {
                fake_registry_snapshot(
                    value,
                    state
                        .cursor_scheme_types
                        .get(target_scheme)
                        .copied()
                        .unwrap_or(1),
                )
            }),
            scheme_name: target_scheme.to_owned(),
        })
    }

    fn write_cursor_slots(&self, values: &[Option<String>; 17]) -> io::Result<()> {
        self.commit_span("write_cursor_slots", || ());
        let mut state = self.state.lock().unwrap();
        if state.fail_cursor_write {
            return Err(io::Error::other("fake cursor registry write failure"));
        }
        for (slot, value) in values.iter().enumerate() {
            if let Some(value) = value {
                state.cursor_slots[slot] = Some(value.clone());
                state.cursor_slot_types[slot] = Some(2);
            }
        }
        Ok(())
    }

    fn write_cursor_scheme(&self, name: &str, values: &[Option<String>; 17]) -> io::Result<()> {
        self.commit_span("write_cursor_scheme", || ());
        let mut state = self.state.lock().unwrap();
        if state.fail_cursor_write {
            return Err(io::Error::other("fake cursor scheme write failure"));
        }
        state
            .cursor_schemes
            .insert(name.to_owned(), join_scheme_fields(values));
        state.cursor_scheme_types.insert(name.to_owned(), 1);
        Ok(())
    }

    fn write_cursor_default_scheme(&self, name: &str) -> io::Result<()> {
        self.commit_span("write_cursor_default_scheme", || ());
        let mut state = self.state.lock().unwrap();
        if state.fail_cursor_write {
            return Err(io::Error::other("fake cursor default write failure"));
        }
        state.cursor_default = Some(name.to_owned());
        state.cursor_default_type = Some(1);
        Ok(())
    }

    fn restore_cursors(&self, snapshot: &CursorSnapshot) -> io::Result<()> {
        self.commit_span("restore_cursors", || ());
        let mut state = self.state.lock().unwrap();
        for (slot, value) in snapshot.slots.iter().enumerate() {
            state.cursor_slots[slot] = value.as_ref().map(fake_registry_string);
            state.cursor_slot_types[slot] = value.as_ref().map(|value| value.value_type);
        }
        state.cursor_default = snapshot.default_scheme.as_ref().map(fake_registry_string);
        state.cursor_default_type = snapshot
            .default_scheme
            .as_ref()
            .map(|value| value.value_type);
        match &snapshot.target_scheme {
            Some(value) => {
                state
                    .cursor_schemes
                    .insert(snapshot.scheme_name.clone(), fake_registry_string(value));
                state
                    .cursor_scheme_types
                    .insert(snapshot.scheme_name.clone(), value.value_type);
            }
            None => {
                state.cursor_schemes.remove(&snapshot.scheme_name);
                state.cursor_scheme_types.remove(&snapshot.scheme_name);
            }
        }
        Ok(())
    }

    fn publish_cursor_directory(&self, source: &Path, id: &str) -> io::Result<PathBuf> {
        self.commit_span("publish_cursor_directory", || ());
        let destination = self.paths.cursor_root.join(id);
        super::transaction::publish_directory_atomically(source, &destination)?;
        self.state
            .lock()
            .unwrap()
            .cursor_dirs
            .push(destination.clone());
        Ok(destination)
    }

    fn remove_cursor_directory(&self, directory: &Path) -> io::Result<()> {
        self.commit_span("remove_cursor_directory", || ());
        match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn refresh_cursors(&self) -> io::Result<()> {
        self.commit_span("refresh_cursors", || ());
        let mut state = self.state.lock().unwrap();
        state.spi_calls += 1;
        if state.fail_spi {
            Err(io::Error::other("fake SPI_SETCURSORS failure"))
        } else {
            Ok(())
        }
    }

    fn modify_shortcut_icons(
        &self,
        changes: &[(PathBuf, PathBuf)],
    ) -> io::Result<Vec<io::Result<()>>> {
        Ok(changes
            .iter()
            .map(|(link, icon)| {
                self.commit_span("modify_shortcut_icon", || ());
                let mut state = self.state.lock().unwrap();
                if state
                    .link_modify_failures
                    .iter()
                    .any(|failure| failure == link)
                {
                    return Err(io::Error::other(format!(
                        "simulated link modification failure: {}",
                        link.display()
                    )));
                }
                state.modified_links.push((link.clone(), icon.clone()));
                Ok(())
            })
            .collect())
    }

    fn notify_shortcuts(&self, _links: &[PathBuf]) -> io::Result<()> {
        self.commit_span("notify_shortcuts", || ());
        let state = self.state.lock().unwrap();
        if state.fail_notify {
            return Err(io::Error::other("fake shortcut notification failure"));
        }
        Ok(())
    }

    fn shell_is_running(&self) -> io::Result<bool> {
        self.commit_span("shell_is_running", || ());
        let state = self.state.lock().unwrap();
        Ok(state.shell_running)
    }

    fn stop_shell(&self) -> io::Result<()> {
        self.commit_span("stop_shell", || ());
        let mut state = self.state.lock().unwrap();
        if state.shell_stop_fails {
            return Err(io::Error::other("fake Explorer stop failure"));
        }
        state.shell_events.push("stop".to_owned());
        state.shell_running = false;
        Ok(())
    }

    fn start_shell(&self) -> io::Result<()> {
        self.commit_span("start_shell", || ());
        let mut state = self.state.lock().unwrap();
        if state.shell_start_fails {
            return Err(io::Error::other("fake Explorer start failure"));
        }
        state.shell_events.push("start".to_owned());
        state.shell_running = true;
        Ok(())
    }

    fn with_temporary_write_permission(
        &self,
        file: &Path,
        operation: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<()> {
        self.commit_span("with_temporary_write_permission", || ());
        self.state
            .lock()
            .unwrap()
            .temporary_permission_targets
            .push(file.to_owned());
        operation()
    }
}

fn fake_registry_snapshot(value: &str, value_type: u32) -> RegistryValueSnapshot {
    let data = value
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    RegistryValueSnapshot { value_type, data }
}

fn fake_registry_string(value: &RegistryValueSnapshot) -> String {
    let mut units = value
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    if units.last() == Some(&0) {
        units.pop();
    }
    String::from_utf16_lossy(&units)
}

pub fn join_scheme_fields(values: &[Option<String>; 17]) -> String {
    values
        .iter()
        .map(|value| value.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

pub fn fs_canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

pub fn path_from_archive_name(root: &Path, archive_path: &str) -> PathBuf {
    let mut path = root.to_owned();
    for component in archive_path.split('/').filter(|part| !part.is_empty()) {
        path.push(component);
    }
    path
}
