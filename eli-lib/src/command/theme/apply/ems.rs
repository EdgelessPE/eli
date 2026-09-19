// `.ems` 鼠标样式资源包。
//
// 归档根目录必须提供原版主题所需的 15 个基础逻辑槽位，每个槽位恰好选择一个
// `.ani` 或 `.cur` 文件（两种格式同时存在时优先 `.ani` 并打印警告）。可选扩展
// 槽位 `aero_pin`/`aero_person` 按主题规范处理；缺失时不修改对应当前值。目录
// 和方案名使用可靠唯一 ID，不使用旧版 `DDHHMMSS` 命名。

use std::io;
use std::path::Path;

use super::archive::{EMS_LIMITS, validate_listing};
use super::transaction::unique_cursor_id;
use super::{CursorSnapshot, ThemeBackend, ThemePaths};

/// 15 个基础槽位：文件基名 → 注册表当前值名称。
pub const BASE_SLOTS: [(&str, &str); 15] = [
    ("aero_arrow", "Arrow"),
    ("aero_helpsel", "Help"),
    ("aero_working", "AppStarting"),
    ("aero_busy", "Wait"),
    ("aero_cross", "Crosshair"),
    ("aero_beam", "IBeam"),
    ("aero_pen", "NWPen"),
    ("aero_unavail", "No"),
    ("aero_ns", "SizeNS"),
    ("aero_ew", "SizeWE"),
    ("aero_nwse", "SizeNWSE"),
    ("aero_nesw", "SizeNESW"),
    ("aero_move", "SizeAll"),
    ("aero_up", "UpArrow"),
    ("aero_link", "Hand"),
];

/// 可选扩展槽位：文件基名 → 注册表当前值名称。
pub const OPTIONAL_SLOTS: [(&str, &str); 2] = [("aero_pin", "Pin"), ("aero_person", "Person")];

/// 视作“可执行文件/脚本”并直接导致预检失败的扩展名。
const REJECTED_EXTENSIONS: [&str; 17] = [
    "exe", "com", "bat", "cmd", "ps1", "vbs", "js", "wsf", "lnk", "msi", "scr", "pif", "jar", "py",
    "wcs", "reg", "dll",
];

/// EMS 预检结果。
#[derive(Debug, Clone)]
pub struct PreparedEms {
    /// staging 中只包含选中文件的目录（将作为光标目录发布）。
    pub source_dir: std::path::PathBuf,
    /// 槽位 → 选中文件名（`.ani`/`.cur`）；可选槽位缺失时为 None。
    pub selected: [Option<String>; 17],
    pub warnings: Vec<String>,
    pub paths: ThemePaths,
}

/// 预检：映射 15 基础 + 可选槽位，拒绝子目录/可执行文件/链接，并验证光标可加载。
pub fn prepare_ems_from_archive(
    archive: &Path,
    staging: &Path,
    paths: &ThemePaths,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedEms> {
    let entries = backend.list_archive(archive)?;
    validate_listing(&entries, &EMS_LIMITS)?;
    for entry in &entries {
        if entry.path.contains('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "EMS archive must not contain subdirectories: {}",
                    entry.path
                ),
            ));
        }
    }
    let mut selected = std::array::from_fn(|_| None);
    let mut warnings = Vec::new();
    let mut recognized = Vec::new();
    for (slot, (base, _)) in BASE_SLOTS.iter().enumerate() {
        let file = select_cursor(
            &entries,
            base,
            slot + 1,
            false,
            &mut warnings,
            &mut recognized,
        )?;
        let Some(file) = file else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "mouse style package is missing a required cursor for slot {} ({base})",
                    slot + 1
                ),
            ));
        };
        selected[slot] = Some(file);
    }
    for (offset, (base, _)) in OPTIONAL_SLOTS.iter().enumerate() {
        let slot = 15 + offset;
        if let Some(file) = select_cursor(
            &entries,
            base,
            slot + 1,
            true,
            &mut warnings,
            &mut recognized,
        )? {
            selected[slot] = Some(file);
        }
    }
    for entry in &entries {
        if recognized
            .iter()
            .any(|name| super::transaction::name_matches(name, &entry.path))
        {
            continue;
        }
        let extension = entry
            .path
            .rsplit('.')
            .next()
            .map(|part| part.to_ascii_lowercase())
            .unwrap_or_default();
        if REJECTED_EXTENSIONS
            .iter()
            .any(|rejected| *rejected == extension)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "EMS archive contains an executable/script/link entry: {}",
                    entry.path
                ),
            ));
        }
        warnings.push(format!(
            "ignored an unrecognized entry in the mouse style package: {}",
            entry.path
        ));
    }
    let whitelist = selected.iter().flatten().cloned().collect::<Vec<_>>();
    backend.extract_archive_entries(archive, staging, &whitelist)?;
    for (slot, file_name) in selected.iter().enumerate() {
        if let Some(file_name) = file_name {
            backend
                .validate_cursor_file(&staging.join(file_name))
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "mouse style slot {} (`{file_name}`) failed to load: {error}",
                            slot + 1
                        ),
                    )
                })?;
        }
    }
    Ok(PreparedEms {
        source_dir: staging.to_owned(),
        selected,
        warnings,
        paths: paths.clone(),
    })
}

fn select_cursor(
    entries: &[super::archive::ArchiveEntry],
    base: &str,
    slot: usize,
    optional: bool,
    warnings: &mut Vec<String>,
    recognized: &mut Vec<String>,
) -> io::Result<Option<String>> {
    let ani = entries
        .iter()
        .find(|entry| {
            !entry.is_directory
                && super::transaction::name_matches(&entry.path, &format!("{base}.ani"))
        })
        .map(|entry| entry.path.clone());
    let cur = entries
        .iter()
        .find(|entry| {
            !entry.is_directory
                && super::transaction::name_matches(&entry.path, &format!("{base}.cur"))
        })
        .map(|entry| entry.path.clone());
    for name in [ani.clone(), cur.clone()].into_iter().flatten() {
        if !recognized.iter().any(|existing| existing == &name) {
            recognized.push(name);
        }
    }
    let selected = match (ani, cur) {
        (Some(ani), Some(cur)) => {
            warnings.push(format!(
                "mouse style slot {slot} has both .ani and .cur; using {ani} and ignoring {cur}"
            ));
            Some(ani)
        }
        (Some(ani), None) => Some(ani),
        (None, Some(cur)) => Some(cur),
        (None, None) if optional => None,
        (None, None) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "mouse style package is missing a required cursor for slot {slot} ({base})"
                ),
            ));
        }
    };
    Ok(selected)
}

/// 方案字符串（17 槽逗号分隔；缺失槽为空字段）。
pub fn scheme_field_string(values: &[Option<String>; 17]) -> String {
    values
        .iter()
        .map(|value| value.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",")
}

fn expand_cursor_path(id: &str, file_name: &str) -> String {
    format!("%SystemRoot%\\Cursors\\Edgeless\\{id}\\{file_name}")
}

/// 提交 EMS：发布目录、写注册表、刷新光标；任一写入或 SPI 失败时恢复快照。
pub fn commit_ems(prepared: &PreparedEms, backend: &dyn ThemeBackend) -> io::Result<Vec<String>> {
    backend.verify_shell_context().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to verify the cursor Shell context: {error}"),
        )
    })?;
    let id = unique_cursor_id(&prepared.paths);
    let published = backend
        .publish_cursor_directory(&prepared.source_dir, &id)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to publish the cursor directory: {error}"),
            )
        })?;
    let snapshot = match backend.snapshot_cursors(&id) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let error = io::Error::new(
                error.kind(),
                format!("failed to snapshot the cursor registry state: {error}"),
            );
            return match backend.remove_cursor_directory(&published) {
                Ok(()) => Err(error),
                Err(remove_error) => Err(io::Error::new(
                    error.kind(),
                    format!(
                        "{error}; removing the newly published cursor directory also failed: {remove_error}"
                    ),
                )),
            };
        }
    };
    let registration = (|| {
        let mut values = std::array::from_fn(|_| None);
        for (slot, file_name) in prepared.selected.iter().enumerate() {
            if let Some(file_name) = file_name {
                values[slot] = Some(expand_cursor_path(&id, file_name));
            }
        }
        backend.write_cursor_slots(&values)?;
        backend.write_cursor_scheme(&id, &values)?;
        backend.write_cursor_default_scheme(&id)?;
        Ok(())
    })();
    if let Err(error) = registration {
        return Err(finish_ems_rollback(backend, error, &snapshot, &published));
    }
    if let Err(error) = backend.refresh_cursors() {
        return Err(finish_ems_rollback(backend, error, &snapshot, &published));
    }
    Ok(prepared.warnings.clone())
}

fn finish_ems_rollback(
    backend: &dyn ThemeBackend,
    error: io::Error,
    snapshot: &CursorSnapshot,
    published: &Path,
) -> io::Error {
    let mut failures = Vec::new();
    if let Err(restore_error) = backend.restore_cursors(snapshot) {
        failures.push(format!("restore registry: {restore_error}"));
    }
    if let Err(remove_error) = backend.remove_cursor_directory(published) {
        failures.push(format!("restore cursor directory: {remove_error}"));
    }
    if failures.is_empty() {
        // 恢复成功后再次调用 SPI_SETCURSORS，尝试恢复旧显示。
        if let Err(refresh_error) = backend.refresh_cursors() {
            failures.push(format!("restore cursor display: {refresh_error}"));
        }
    }
    if failures.is_empty() {
        error
    } else {
        io::Error::new(
            error.kind(),
            format!("{error}; EMS rollback also failed: {}", failures.join("; ")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::details::archive::ArchiveEntry;
    use crate::command::theme::apply::test_support::{FakeBackend, test_root};

    fn archive_entry(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 1,
            is_directory: false,
            encrypted: false,
            is_link: false,
        }
    }

    fn paths_at(root: &Path) -> ThemePaths {
        ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("Users/Theme/eli/staging"),
            wallpaper_dir: root.join("Users/Theme/eli/wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: vec![],
            icon_cache_dir: root.join("Cache"),
        }
    }

    #[test]
    fn scheme_fields_keep_missing_optional_slots_empty() {
        let mut values = std::array::from_fn(|_| None);
        values[0] = Some("a.cur".to_owned());

        let joined = scheme_field_string(&values);

        assert_eq!(joined, "a.cur,,,,,,,,,,,,,,,,");
        assert_eq!(joined.split(',').count(), 17);
    }

    #[test]
    fn expands_cursor_paths_with_system_root() {
        assert_eq!(
            expand_cursor_path("c-1", "aero_arrow.ani"),
            "%SystemRoot%\\Cursors\\Edgeless\\c-1\\aero_arrow.ani"
        );
    }

    #[test]
    fn base_slot_mapping_matches_the_specification() {
        let expected = [
            ("aero_arrow", "Arrow"),
            ("aero_helpsel", "Help"),
            ("aero_working", "AppStarting"),
            ("aero_busy", "Wait"),
            ("aero_cross", "Crosshair"),
            ("aero_beam", "IBeam"),
            ("aero_pen", "NWPen"),
            ("aero_unavail", "No"),
            ("aero_ns", "SizeNS"),
            ("aero_ew", "SizeWE"),
            ("aero_nwse", "SizeNWSE"),
            ("aero_nesw", "SizeNESW"),
            ("aero_move", "SizeAll"),
            ("aero_up", "UpArrow"),
            ("aero_link", "Hand"),
        ];
        assert_eq!(BASE_SLOTS, expected);
        assert_eq!(
            OPTIONAL_SLOTS,
            [("aero_pin", "Pin"), ("aero_person", "Person")]
        );
    }

    #[test]
    fn selects_ani_before_cur_and_reports_the_ignored_variant() {
        let entries = [
            archive_entry("AERO_ARROW.CUR"),
            archive_entry("aero_arrow.ani"),
        ];
        let mut warnings = Vec::new();
        let mut recognized = Vec::new();

        let selected = select_cursor(
            &entries,
            "aero_arrow",
            1,
            false,
            &mut warnings,
            &mut recognized,
        )
        .unwrap();

        assert_eq!(selected.as_deref(), Some("aero_arrow.ani"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("AERO_ARROW.CUR"));
        assert_eq!(recognized.len(), 2);
    }

    #[test]
    fn rejects_a_missing_required_cursor_but_allows_a_missing_optional_cursor() {
        let mut warnings = Vec::new();
        let mut recognized = Vec::new();

        let error =
            select_cursor(&[], "aero_arrow", 1, false, &mut warnings, &mut recognized).unwrap_err();
        assert!(error.to_string().contains("missing a required cursor"));
        assert!(
            select_cursor(&[], "aero_pin", 16, true, &mut warnings, &mut recognized,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn commit_publishes_optional_pin_and_person_slots() {
        let root = test_root("optional");
        let backend = FakeBackend::new(root.clone());
        let paths = paths_at(&root);
        let staging = root.join("staging/ems");
        std::fs::create_dir_all(&staging).unwrap();
        for name in [
            "aero_arrow.ani",
            "aero_helpsel.cur",
            "aero_working.ani",
            "aero_busy.cur",
            "aero_cross.ani",
            "aero_beam.cur",
            "aero_pen.ani",
            "aero_unavail.cur",
            "aero_ns.ani",
            "aero_ew.cur",
            "aero_nwse.ani",
            "aero_nesw.cur",
            "aero_move.ani",
            "aero_up.cur",
            "aero_link.ani",
            "aero_pin.cur",
            "aero_person.ani",
        ] {
            std::fs::write(staging.join(name), "cursor").unwrap();
        }
        let prepared = PreparedEms {
            source_dir: staging,
            selected: {
                let mut selected = std::array::from_fn(|_| None);
                let names = [
                    Some("aero_arrow.ani".to_owned()),
                    Some("aero_helpsel.cur".to_owned()),
                    Some("aero_working.ani".to_owned()),
                    Some("aero_busy.cur".to_owned()),
                    Some("aero_cross.ani".to_owned()),
                    Some("aero_beam.cur".to_owned()),
                    Some("aero_pen.ani".to_owned()),
                    Some("aero_unavail.cur".to_owned()),
                    Some("aero_ns.ani".to_owned()),
                    Some("aero_ew.cur".to_owned()),
                    Some("aero_nwse.ani".to_owned()),
                    Some("aero_nesw.cur".to_owned()),
                    Some("aero_move.ani".to_owned()),
                    Some("aero_up.cur".to_owned()),
                    Some("aero_link.ani".to_owned()),
                    Some("aero_pin.cur".to_owned()),
                    Some("aero_person.ani".to_owned()),
                ];
                for (slot, name) in names.into_iter().enumerate() {
                    selected[slot] = name;
                }
                selected
            },
            warnings: vec![],
            paths,
        };

        let result = commit_ems(&prepared, &backend).unwrap();
        assert!(result.is_empty());

        let state = backend.state.lock().unwrap();
        let id = directory_id(&state);
        assert_eq!(state.spi_calls, 1);
        let expected_pin = format!("%SystemRoot%\\Cursors\\Edgeless\\{id}\\aero_pin.cur");
        let expected_person = format!("%SystemRoot%\\Cursors\\Edgeless\\{id}\\aero_person.ani");
        assert_eq!(state.cursor_slots[15], Some(expected_pin));
        assert_eq!(state.cursor_slots[16], Some(expected_person));
        assert_eq!(state.cursor_dirs.len(), 1);
        assert_eq!(state.cursor_default.as_deref(), Some(id.as_str()));
        std::fs::remove_dir_all(root).unwrap();
    }

    fn directory_id(state: &crate::command::theme::apply::test_support::FakeState) -> String {
        state
            .cursor_dirs
            .first()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn commit_rolls_back_registry_and_directory_when_a_write_fails() {
        let root = test_root("rollback");
        let backend = FakeBackend::new(root.clone());
        let paths = paths_at(&root);
        let staging = root.join("staging/ems");
        std::fs::create_dir_all(&staging).unwrap();
        for name in [
            "aero_arrow.ani",
            "aero_helpsel.cur",
            "aero_working.ani",
            "aero_busy.cur",
            "aero_cross.ani",
            "aero_beam.cur",
            "aero_pen.ani",
            "aero_unavail.cur",
            "aero_ns.ani",
            "aero_ew.cur",
            "aero_nwse.ani",
            "aero_nesw.cur",
            "aero_move.ani",
            "aero_up.cur",
            "aero_link.ani",
        ] {
            std::fs::write(staging.join(name), "cursor").unwrap();
        }
        let prepared = PreparedEms {
            source_dir: staging,
            selected: {
                let mut selected = std::array::from_fn(|_| None);
                for (slot, (base, _)) in BASE_SLOTS.iter().enumerate() {
                    selected[slot] = Some(format!("{base}.ani"));
                }
                selected
            },
            warnings: vec![],
            paths: paths.clone(),
        };
        backend.state.lock().unwrap().fail_cursor_write = true;

        let error = commit_ems(&prepared, &backend).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("fake cursor registry write failure")
        );

        let state = backend.state.lock().unwrap();
        assert!(state.cursor_slots.iter().all(Option::is_none));
        assert!(state.cursor_schemes.is_empty());
        assert_eq!(state.cursor_default, None);
        // 发布目录被回滚移除（fake 的记录保留历史，磁盘上不再存在）。
        assert_eq!(state.cursor_dirs.len(), 1);
        assert!(!state.cursor_dirs[0].exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commit_rolls_back_registry_and_directory_when_cursor_refresh_fails() {
        let root = test_root("spi");
        let backend = FakeBackend::new(root.clone());
        let paths = paths_at(&root);
        let staging = root.join("staging/ems");
        std::fs::create_dir_all(&staging).unwrap();
        let mut selected = std::array::from_fn(|_| None);
        for (slot, (base, _)) in BASE_SLOTS.iter().enumerate() {
            let name = format!("{base}.ani");
            std::fs::write(staging.join(&name), "cursor").unwrap();
            selected[slot] = Some(name);
        }
        let prepared = PreparedEms {
            source_dir: staging,
            selected,
            warnings: vec![],
            paths,
        };
        {
            let mut state = backend.state.lock().unwrap();
            state.cursor_slots[0] = Some("old-arrow.cur".to_owned());
            state.cursor_slot_types[0] = Some(1);
            state.cursor_default = Some("old-scheme".to_owned());
            state.cursor_default_type = Some(2);
            state.fail_spi = true;
        }

        let error = commit_ems(&prepared, &backend).unwrap_err();

        assert!(error.to_string().contains("SPI_SETCURSORS"));
        let state = backend.state.lock().unwrap();
        assert_eq!(state.cursor_slots[0].as_deref(), Some("old-arrow.cur"));
        assert_eq!(state.cursor_slot_types[0], Some(1));
        assert_eq!(state.cursor_default.as_deref(), Some("old-scheme"));
        assert_eq!(state.cursor_default_type, Some(2));
        assert!(state.cursor_schemes.is_empty());
        assert_eq!(state.spi_calls, 2);
        assert_eq!(state.cursor_dirs.len(), 1);
        assert!(!state.cursor_dirs[0].exists());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_failure_removes_the_published_cursor_directory() {
        let root = test_root("snapshot");
        let backend = FakeBackend::new(root.clone());
        let paths = paths_at(&root);
        let staging = root.join("staging/ems");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("aero_arrow.cur"), "cursor").unwrap();
        let prepared = PreparedEms {
            source_dir: staging,
            selected: std::array::from_fn(|slot| (slot == 0).then(|| "aero_arrow.cur".to_owned())),
            warnings: vec![],
            paths,
        };
        backend.state.lock().unwrap().fail_cursor_snapshot = true;

        let error = commit_ems(&prepared, &backend).unwrap_err();

        assert!(error.to_string().contains("cursor snapshot"));
        let state = backend.state.lock().unwrap();
        assert_eq!(state.cursor_dirs.len(), 1);
        assert!(!state.cursor_dirs[0].exists());
        assert!(
            state
                .log
                .iter()
                .any(|event| event == "remove_cursor_directory")
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
