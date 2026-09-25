// `eli theme store`：按旧版 instTheme.cmd 契约把主题持久化到启动盘。
//
// 当前版本故意保留旧版 LoadScreen 目录规范：`.els` 解包到
// `Edgeless/Default/LoadScreen`。未来迁移到新版规范时，应在这里增加显式映射，
// 不能改变本版兼容路径的含义。

use crate::Ctx;
use crate::dependency::ProgramDependency;
use encoding_rs::GBK;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::apply::details::archive::{
    ArchiveEntry, EIS_LIMITS, ELS_LIMITS, EMS_LIMITS, ESS_LIMITS, ETH_LIMITS, parse_listing,
    validate_listing, verify_extraction,
};
use super::apply::details::transaction::unique_transaction_id;
use super::apply::{ThemeComponent, ThemeType, identify_package};

const INFO_LINES: usize = 5;

/// 主题保存结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSummary {
    pub source: PathBuf,
    pub kind: ThemeType,
    pub bootdisk: PathBuf,
    pub stored: Vec<ThemeComponent>,
    pub default_replaced: bool,
    pub wallpaper_backed_up: bool,
}

/// 把主题包、资源包或壁纸保存为所选启动盘的默认主题。
pub fn store(ctx: &Ctx, package: &Path) -> io::Result<StoreSummary> {
    let kind = identify_package(package)?;
    validate_source(package)?;
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let seven_zip = if needs_seven_zip(kind) {
        let programs = ctx
            .dependencies()
            .require_programs(&[ProgramDependency::SevenZip])?;
        Some(programs.executable(ProgramDependency::SevenZip)?.to_owned())
    } else {
        None
    };
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    let edgeless = bootdisk.mount_point.join("Edgeless");
    let mut summary = store_at(&edgeless, package, kind, seven_zip.as_deref())?;
    summary.bootdisk = bootdisk.mount_point.clone();
    Ok(summary)
}

fn needs_seven_zip(kind: ThemeType) -> bool {
    matches!(
        kind,
        ThemeType::Eth | ThemeType::Eis | ThemeType::Ems | ThemeType::Ess | ThemeType::Els
    )
}

fn validate_source(package: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(package).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to inspect theme source {}: {error}",
                package.display()
            ),
        )
    })?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("theme source is not a regular file: {}", package.display()),
        ));
    }
    Ok(())
}

fn store_at(
    edgeless: &Path,
    package: &Path,
    kind: ThemeType,
    seven_zip: Option<&Path>,
) -> io::Result<StoreSummary> {
    ensure_real_directory(edgeless)?;
    recover_interrupted_transactions(edgeless)?;
    let staging = StagingDirectory::new(edgeless)?;
    let old_info = read_info(&edgeless.join("Default/Info.txt"))?;
    let name = package_stem(package)?;
    let archive_snapshot = if needs_seven_zip(kind) {
        let file_name = package.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "theme source has no file name")
        })?;
        let snapshot = staging.path.join("input").join(file_name);
        copy_file(package, &snapshot)?;
        Some(snapshot)
    } else {
        None
    };
    let prepared_package = archive_snapshot.as_deref().unwrap_or(package);
    let mut plan = StorePlan::default();

    match kind {
        ThemeType::Eth => prepare_theme_pack(
            prepared_package,
            required_seven_zip(seven_zip)?,
            edgeless,
            &staging,
            old_info,
            &name,
            &mut plan,
        )?,
        ThemeType::Eis | ThemeType::Ems | ThemeType::Ess => {
            let component = resource_component(kind);
            validate_resource_archive(required_seven_zip(seven_zip)?, prepared_package, component)?;
            prepare_resource_pack(
                prepared_package,
                edgeless,
                &staging,
                old_info,
                &name,
                component,
                &mut plan,
            )?
        }
        ThemeType::Esc => prepare_esc(package, edgeless, &staging, old_info, &name, &mut plan)?,
        ThemeType::Els => prepare_loadscreen(
            prepared_package,
            required_seven_zip(seven_zip)?,
            edgeless,
            &staging,
            old_info,
            &name,
            &mut plan,
        )?,
        ThemeType::Jpg => prepare_wallpaper(package, edgeless, &staging, &mut plan)?,
    }

    reject_source_destination_alias(package, &plan.changes)?;
    publish_plan(edgeless, &staging, &plan.changes)?;
    let wallpaper_backed_up = plan.changes.iter().any(|change| {
        change.destination == edgeless.join("wp_backup.jpg") && change.staged.is_some()
    });
    Ok(StoreSummary {
        source: package.to_owned(),
        kind,
        bootdisk: PathBuf::new(),
        stored: plan.stored,
        default_replaced: plan.default_replaced,
        wallpaper_backed_up,
    })
}

fn required_seven_zip(seven_zip: Option<&Path>) -> io::Result<&Path> {
    seven_zip.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "7-Zip is required"))
}

#[derive(Default)]
struct StorePlan {
    changes: Vec<PublishChange>,
    stored: Vec<ThemeComponent>,
    default_replaced: bool,
}

struct PublishChange {
    staged: Option<PathBuf>,
    destination: PathBuf,
}

fn prepare_theme_pack(
    package: &Path,
    seven_zip: &Path,
    edgeless: &Path,
    staging: &StagingDirectory,
    mut info: ThemeInfo,
    theme_name: &str,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let entries = list_archive(seven_zip, package)?;
    validate_listing(&entries, &ETH_LIMITS)?;
    let components = theme_components(&entries)?;
    let unpacked = staging.path.join("theme");
    fs::create_dir_all(&unpacked)?;
    let whitelist = components
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    if !whitelist.is_empty() {
        extract_archive(seven_zip, package, &unpacked, Some(&whitelist))?;
        verify_extraction(&unpacked)?;
    }

    let new_default = staging.path.join("new-default");
    fs::create_dir_all(&new_default)?;
    let mut wallpaper = None;
    for (component, entry) in components {
        let source = unpacked.join(&entry);
        match component {
            ThemeComponent::Wallpaper => {
                let destination = staging.path.join("new-wallpaper.jpg");
                copy_file(&source, &destination)?;
                wallpaper = Some(destination);
            }
            ThemeComponent::LoadScreen => {
                let destination = new_default.join("LoadScreen");
                prepare_loadscreen_directory(seven_zip, &source, &destination, None)?;
                info.set(component, theme_name);
            }
            ThemeComponent::IconPack
            | ThemeComponent::MouseStyle
            | ThemeComponent::SystemIconPack => {
                validate_resource_archive(seven_zip, &source, component)?;
                copy_file(&source, &new_default.join(component.display_name()))?;
                info.set(component, theme_name);
            }
            ThemeComponent::StartIsBackConfig => {
                copy_file(&source, &new_default.join(component.display_name()))?;
                info.set(component, theme_name);
            }
        }
        plan.stored.push(component);
    }
    write_info(&new_default.join("Info.txt"), &info)?;
    plan.changes.push(PublishChange {
        staged: Some(new_default),
        destination: edgeless.join("Default"),
    });
    plan.default_replaced = true;
    prepare_wallpaper_changes(edgeless, staging, wallpaper, plan)?;
    Ok(())
}

fn prepare_resource_pack(
    package: &Path,
    edgeless: &Path,
    staging: &StagingDirectory,
    mut info: ThemeInfo,
    name: &str,
    component: ThemeComponent,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let staged = staging.path.join(component.display_name());
    copy_file(package, &staged)?;
    info.set(component, name);
    prepare_component_and_info(edgeless, staging, staged, component, info, plan)
}

fn prepare_esc(
    package: &Path,
    edgeless: &Path,
    staging: &StagingDirectory,
    mut info: ThemeInfo,
    name: &str,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let component = ThemeComponent::StartIsBackConfig;
    let staged = staging.path.join(component.display_name());
    copy_file(package, &staged)?;
    info.set(component, name);
    prepare_component_and_info(edgeless, staging, staged, component, info, plan)
}

fn prepare_loadscreen(
    package: &Path,
    seven_zip: &Path,
    edgeless: &Path,
    staging: &StagingDirectory,
    mut info: ThemeInfo,
    name: &str,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let component = ThemeComponent::LoadScreen;
    let destination = staging.path.join("LoadScreen");
    prepare_loadscreen_directory(
        seven_zip,
        package,
        &destination,
        Some(&edgeless.join("Default/LoadScreen")),
    )?;
    info.set(component, name);
    prepare_component_and_info(edgeless, staging, destination, component, info, plan)
}

fn prepare_wallpaper(
    package: &Path,
    edgeless: &Path,
    staging: &StagingDirectory,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let staged = staging.path.join("new-wallpaper.jpg");
    copy_file(package, &staged)?;
    plan.stored.push(ThemeComponent::Wallpaper);
    prepare_wallpaper_changes(edgeless, staging, Some(staged), plan)
}

fn prepare_component_and_info(
    edgeless: &Path,
    staging: &StagingDirectory,
    staged: PathBuf,
    component: ThemeComponent,
    info: ThemeInfo,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let staged_info = staging.path.join("Info.txt");
    write_info(&staged_info, &info)?;
    plan.changes.push(PublishChange {
        staged: Some(staged),
        destination: edgeless
            .join("Default")
            .join(component_destination(component)),
    });
    plan.changes.push(PublishChange {
        staged: Some(staged_info),
        destination: edgeless.join("Default/Info.txt"),
    });
    plan.stored.push(component);
    Ok(())
}

fn component_destination(component: ThemeComponent) -> &'static str {
    match component {
        ThemeComponent::LoadScreen => "LoadScreen",
        _ => component.display_name(),
    }
}

fn prepare_wallpaper_changes(
    edgeless: &Path,
    staging: &StagingDirectory,
    wallpaper: Option<PathBuf>,
    plan: &mut StorePlan,
) -> io::Result<()> {
    let current = edgeless.join("wp.jpg");
    let backup = match fs::symlink_metadata(&current) {
        Ok(metadata) if metadata.is_file() && !is_reparse_point(&metadata) => {
            let staged = staging.path.join("previous-wallpaper.jpg");
            copy_file(&current, &staged)?;
            Some(staged)
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "existing wallpaper is not a regular file: {}",
                    current.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    plan.changes.push(PublishChange {
        staged: backup,
        destination: edgeless.join("wp_backup.jpg"),
    });
    plan.changes.push(PublishChange {
        staged: wallpaper,
        destination: current,
    });
    Ok(())
}

fn resource_component(kind: ThemeType) -> ThemeComponent {
    match kind {
        ThemeType::Eis => ThemeComponent::IconPack,
        ThemeType::Ems => ThemeComponent::MouseStyle,
        ThemeType::Ess => ThemeComponent::SystemIconPack,
        _ => unreachable!("only archive resource packs use this mapping"),
    }
}

fn validate_resource_archive(
    seven_zip: &Path,
    package: &Path,
    component: ThemeComponent,
) -> io::Result<()> {
    let limits = match component {
        ThemeComponent::IconPack => &EIS_LIMITS,
        ThemeComponent::MouseStyle => &EMS_LIMITS,
        ThemeComponent::SystemIconPack => &ESS_LIMITS,
        _ => unreachable!("component is not an archive resource pack"),
    };
    let entries = list_archive(seven_zip, package)?;
    validate_listing(&entries, limits)
}

fn prepare_loadscreen_directory(
    seven_zip: &Path,
    package: &Path,
    destination: &Path,
    existing: Option<&Path>,
) -> io::Result<()> {
    let entries = list_archive(seven_zip, package)?;
    validate_listing(&entries, &ELS_LIMITS)?;
    fs::create_dir_all(destination)?;
    if let Some(existing) = existing
        && existing.exists()
    {
        copy_directory_contents(existing, destination)?;
        remove_top_level_jpegs(destination)?;
    }
    extract_archive(seven_zip, package, destination, None)?;
    verify_extraction(destination)
}

fn copy_directory_contents(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("legacy LoadScreen path is unsafe: {}", source.display()),
        ));
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if is_reparse_point(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "legacy LoadScreen contains a reparse point: {}",
                    source_path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination_path)?;
            copy_directory_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            copy_file(&source_path, &destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "legacy LoadScreen contains an unsupported entry: {}",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn remove_top_level_jpegs(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let is_jpeg = entry
            .path()
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"));
        if metadata.is_file() && !is_reparse_point(&metadata) && is_jpeg {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn theme_components(entries: &[ArchiveEntry]) -> io::Result<Vec<(ThemeComponent, String)>> {
    let mut found = Vec::new();
    for entry in entries {
        if entry.is_directory || entry.path.contains('/') {
            continue;
        }
        let component = match entry.path.to_ascii_lowercase().as_str() {
            "wallpaper.jpg" => Some(ThemeComponent::Wallpaper),
            "loadscreen.els" => Some(ThemeComponent::LoadScreen),
            "iconpack.eis" => Some(ThemeComponent::IconPack),
            "mousestyle.ems" => Some(ThemeComponent::MouseStyle),
            "startisbackconfig.esc" => Some(ThemeComponent::StartIsBackConfig),
            "systemiconpack.ess" => Some(ThemeComponent::SystemIconPack),
            _ => None,
        };
        let Some(component) = component else {
            continue;
        };
        if found
            .iter()
            .any(|(existing, _): &(ThemeComponent, String)| *existing == component)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "theme package contains multiple variants of {}",
                    component.display_name()
                ),
            ));
        }
        found.push((component, entry.path.clone()));
    }
    let mut ordered = Vec::new();
    for component in [
        ThemeComponent::Wallpaper,
        ThemeComponent::LoadScreen,
        ThemeComponent::IconPack,
        ThemeComponent::MouseStyle,
        ThemeComponent::StartIsBackConfig,
        ThemeComponent::SystemIconPack,
    ] {
        if let Some(found) = found.iter().find(|(candidate, _)| *candidate == component) {
            ordered.push(found.clone());
        }
    }
    Ok(ordered)
}

fn list_archive(seven_zip: &Path, package: &Path) -> io::Result<Vec<ArchiveEntry>> {
    let output = run_seven_zip(
        seven_zip,
        &[
            OsString::from("l"),
            OsString::from("-slt"),
            OsString::from("-sccUTF-8"),
            OsString::from("--"),
            package.as_os_str().to_owned(),
        ],
        package,
    )?;
    parse_listing(&output)
}

fn extract_archive(
    seven_zip: &Path,
    package: &Path,
    destination: &Path,
    entries: Option<&[String]>,
) -> io::Result<()> {
    let mut arguments = vec![
        OsString::from("x"),
        OsString::from("-y"),
        OsString::from("-aoa"),
        OsString::from("-bd"),
        OsString::from("-bb0"),
        OsString::from("-spd"),
    ];
    let mut output = OsString::from("-o");
    output.push(destination);
    arguments.push(output);
    arguments.push(OsString::from("--"));
    arguments.push(package.as_os_str().to_owned());
    if let Some(entries) = entries {
        arguments.extend(entries.iter().map(OsString::from));
    }
    run_seven_zip(seven_zip, &arguments, package).map(|_| ())
}

fn run_seven_zip(seven_zip: &Path, arguments: &[OsString], source: &Path) -> io::Result<String> {
    let current = source.parent().unwrap_or_else(|| Path::new("."));
    let output = Command::new(seven_zip)
        .args(arguments)
        .current_dir(current)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to execute {}: {error}", seven_zip.display()),
            )
        })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "7-Zip failed for {} with status {}: {}",
            source.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("7-Zip returned non-UTF-8 output: {error}"),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeInfo {
    lines: [String; INFO_LINES],
}

impl Default for ThemeInfo {
    fn default() -> Self {
        Self {
            lines: [
                "图标资源包：Unknown".to_owned(),
                "系统图标资源包：Unknown".to_owned(),
                "LoadScreen资源包：Unknown".to_owned(),
                "鼠标样式资源包：Unknown".to_owned(),
                "开始菜单样式配置文件：Unknown".to_owned(),
            ],
        }
    }
}

impl ThemeInfo {
    fn set(&mut self, component: ThemeComponent, name: &str) {
        let (index, label) = match component {
            ThemeComponent::IconPack => (0, "图标资源包："),
            ThemeComponent::SystemIconPack => (1, "系统图标资源包："),
            ThemeComponent::LoadScreen => (2, "LoadScreen资源包："),
            ThemeComponent::MouseStyle => (3, "鼠标样式资源包："),
            ThemeComponent::StartIsBackConfig => (4, "开始菜单样式配置文件："),
            ThemeComponent::Wallpaper => return,
        };
        self.lines[index] = format!("{label}{name}");
    }
}

fn read_info(path: &Path) -> io::Result<ThemeInfo> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ThemeInfo::default()),
        Err(error) => return Err(error),
    };
    let text = match String::from_utf8(bytes.clone()) {
        Ok(text) => text,
        Err(_) => {
            let (decoded, _, had_errors) = GBK.decode(&bytes);
            if had_errors {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "theme metadata is neither UTF-8 nor GBK: {}",
                        path.display()
                    ),
                ));
            }
            decoded.into_owned()
        }
    };
    let mut info = ThemeInfo::default();
    for (index, line) in text.lines().take(INFO_LINES).enumerate() {
        let line = line.trim_end_matches('\r');
        info.lines[index] = line.strip_suffix(' ').unwrap_or(line).to_owned();
    }
    Ok(info)
}

fn write_info(path: &Path, info: &ThemeInfo) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Info.txt has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let text = info
        .lines
        .iter()
        .map(|line| format!("{line} \r\n"))
        .collect::<String>();
    let (encoded, _, had_errors) = GBK.encode(&text);
    if had_errors {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "theme metadata contains characters that cannot be encoded as GBK",
        ));
    }
    let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
    output.write_all(&encoded)?;
    output.sync_all()
}

fn package_stem(package: &Path) -> io::Result<String> {
    package
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("theme source has no UTF-8 file stem: {}", package.display()),
            )
        })
}

fn reject_source_destination_alias(source: &Path, changes: &[PublishChange]) -> io::Result<()> {
    let source = fs::canonicalize(source)?;
    for change in changes {
        let Ok(destination) = fs::canonicalize(&change.destination) else {
            continue;
        };
        if destination == source {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "theme source is already the selected boot disk destination: {}",
                    destination.display()
                ),
            ));
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("theme payload is not a regular file: {}", source.display()),
        ));
    }
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "staged file has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()
}

struct StagingDirectory {
    path: PathBuf,
    committed: bool,
}

impl StagingDirectory {
    fn new(edgeless: &Path) -> io::Result<Self> {
        let path = edgeless.join(format!(".eli-theme-store-{}", unique_transaction_id()));
        fs::create_dir(&path)?;
        Ok(Self {
            path,
            committed: false,
        })
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.committed || !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum JournalRecord {
    Change {
        destination: PathBuf,
        backup: PathBuf,
        had_original: bool,
    },
    Committed,
}

fn publish_plan(
    edgeless: &Path,
    staging: &StagingDirectory,
    changes: &[PublishChange],
) -> io::Result<()> {
    let journal_path = staging.path.join("transaction.jsonl");
    let mut journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&journal_path)?;
    let backup_root = staging.path.join("backup");
    fs::create_dir(&backup_root)?;
    let mut applied = Vec::new();
    let result = (|| {
        for (index, change) in changes.iter().enumerate() {
            ensure_destination(edgeless, &change.destination)?;
            let metadata = optional_metadata(&change.destination)?;
            if metadata.as_ref().is_some_and(is_reparse_point) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "theme destination is a reparse point: {}",
                        change.destination.display()
                    ),
                ));
            }
            let had_original = metadata.is_some();
            let backup = backup_root.join(index.to_string());
            write_journal(
                &mut journal,
                &JournalRecord::Change {
                    destination: relative_to(edgeless, &change.destination)?,
                    backup: relative_to(&staging.path, &backup)?,
                    had_original,
                },
            )?;
            if had_original {
                fs::rename(&change.destination, &backup)?;
            }
            applied.push((change.destination.clone(), backup.clone(), had_original));
            if let Some(staged) = &change.staged {
                let parent = change.destination.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent")
                })?;
                fs::create_dir_all(parent)?;
                fs::rename(staged, &change.destination)?;
            }
        }
        write_journal(&mut journal, &JournalRecord::Committed)
    })();
    if let Err(error) = result {
        return match rollback(&applied) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(io::Error::other(format!(
                "{error}; failed to roll back theme store: {rollback_error}"
            ))),
        };
    }
    Ok(())
}

fn write_journal(journal: &mut File, record: &JournalRecord) -> io::Result<()> {
    serde_json::to_writer(&mut *journal, record).map_err(io::Error::other)?;
    journal.write_all(b"\n")?;
    journal.sync_data()
}

fn rollback(changes: &[(PathBuf, PathBuf, bool)]) -> io::Result<()> {
    let mut failures = Vec::new();
    for (destination, backup, had_original) in changes.iter().rev() {
        if let Err(error) = remove_path(destination) {
            failures.push(format!("remove {}: {error}", destination.display()));
        }
        if *had_original && let Err(error) = fs::rename(backup, destination) {
            failures.push(format!(
                "restore {} from {}: {error}",
                destination.display(),
                backup.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

fn recover_interrupted_transactions(edgeless: &Path) -> io::Result<()> {
    for entry in fs::read_dir(edgeless)? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".eli-theme-store-") {
            continue;
        }
        let staging = entry.path();
        let metadata = fs::symlink_metadata(&staging)?;
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            continue;
        }
        let journal_path = staging.join("transaction.jsonl");
        if !journal_path.is_file() {
            fs::remove_dir_all(&staging)?;
            continue;
        }
        let (changes, committed) = read_journal(&journal_path)?;
        if !committed {
            recover_changes(edgeless, &staging, &changes)?;
        }
        fs::remove_dir_all(staging)?;
    }
    Ok(())
}

type RecoveryChange = (PathBuf, PathBuf, bool);

fn read_journal(path: &Path) -> io::Result<(Vec<RecoveryChange>, bool)> {
    let mut changes = Vec::new();
    let mut committed = false;
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<JournalRecord>(&line).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid theme-store transaction {}: {error}",
                    path.display()
                ),
            )
        })? {
            JournalRecord::Change {
                destination,
                backup,
                had_original,
            } => changes.push((destination, backup, had_original)),
            JournalRecord::Committed => committed = true,
        }
    }
    Ok((changes, committed))
}

fn recover_changes(edgeless: &Path, staging: &Path, changes: &[RecoveryChange]) -> io::Result<()> {
    let mut destinations = std::collections::HashSet::new();
    let mut backups = std::collections::HashSet::new();
    for (destination, backup, _) in changes {
        if !is_allowed_recovery_destination(destination)
            || !is_allowed_recovery_backup(backup)
            || !destinations.insert(destination.clone())
            || !backups.insert(backup.clone())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "theme-store transaction contains an unsafe or duplicate path",
            ));
        }
    }
    let mut failures = Vec::new();
    for (destination, backup, had_original) in changes.iter().rev() {
        let destination = safe_join(edgeless, destination)?;
        let backup = safe_join(staging, backup)?;
        ensure_destination(edgeless, &destination)?;
        ensure_destination(staging, &backup)?;
        if *had_original {
            if backup.exists() {
                let metadata = fs::symlink_metadata(&backup)?;
                if is_reparse_point(&metadata) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "theme-store backup is a reparse point: {}",
                            backup.display()
                        ),
                    ));
                }
                if let Err(error) = remove_path(&destination) {
                    failures.push(error.to_string());
                }
                if let Err(error) = fs::rename(&backup, &destination) {
                    failures.push(error.to_string());
                }
            }
        } else if let Err(error) = remove_path(&destination) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

fn is_allowed_recovery_destination(path: &Path) -> bool {
    [
        "Default",
        "Default/Info.txt",
        "Default/IconPack.eis",
        "Default/MouseStyle.ems",
        "Default/StartIsBackConfig.esc",
        "Default/SystemIconPack.ess",
        "Default/LoadScreen",
        "wp.jpg",
        "wp_backup.jpg",
    ]
    .iter()
    .any(|allowed| path == Path::new(allowed))
}

fn is_allowed_recovery_backup(path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix("backup") else {
        return false;
    };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(index)) = components.next() else {
        return false;
    };
    components.next().is_none()
        && index.to_str().is_some_and(|index| {
            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn ensure_destination(root: &Path, destination: &Path) -> io::Result<()> {
    let relative = destination.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "theme destination escapes Edgeless: {}",
                destination.display()
            ),
        )
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe theme destination: {}", destination.display()),
        ));
    }
    let mut current = root.to_owned();
    for component in relative
        .components()
        .take(relative.components().count().saturating_sub(1))
    {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !is_reparse_point(&metadata) => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "theme destination ancestor is unsafe: {}",
                        current.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn relative_to(root: &Path, path: &Path) -> io::Result<PathBuf> {
    path.strip_prefix(root).map(Path::to_owned).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is outside transaction root: {}", path.display()),
        )
    })
}

fn safe_join(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsafe transaction path: {}", relative.display()),
        ));
    }
    Ok(root.join(relative))
}

fn optional_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !is_reparse_point(&metadata) => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_real_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Edgeless path is not a safe directory: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stores_a_standalone_esc_and_updates_only_its_info_line() {
        let root = test_root("esc");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(edgeless.join("Default")).unwrap();
        let mut info = ThemeInfo::default();
        info.set(ThemeComponent::IconPack, "OldIcons");
        write_info(&edgeless.join("Default/Info.txt"), &info).unwrap();
        let source = root.join("Blue.esc");
        fs::write(&source, b"pecmd script").unwrap();

        let summary = store_at(&edgeless, &source, ThemeType::Esc, None).unwrap();

        assert_eq!(summary.stored, vec![ThemeComponent::StartIsBackConfig]);
        assert_eq!(
            fs::read(edgeless.join("Default/StartIsBackConfig.esc")).unwrap(),
            b"pecmd script"
        );
        let stored = read_info(&edgeless.join("Default/Info.txt")).unwrap();
        assert_eq!(stored.lines[0], "图标资源包：OldIcons");
        assert_eq!(stored.lines[4], "开始菜单样式配置文件：Blue");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallpaper_replacement_rotates_the_previous_wallpaper() {
        let root = test_root("wallpaper");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(&edgeless).unwrap();
        fs::write(edgeless.join("wp.jpg"), b"old").unwrap();
        fs::write(edgeless.join("wp_backup.jpg"), b"stale").unwrap();
        let source = root.join("new.jpg");
        fs::write(&source, b"new").unwrap();

        let summary = store_at(&edgeless, &source, ThemeType::Jpg, None).unwrap();

        assert!(summary.wallpaper_backed_up);
        assert_eq!(fs::read(edgeless.join("wp.jpg")).unwrap(), b"new");
        assert_eq!(fs::read(edgeless.join("wp_backup.jpg")).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn info_is_written_as_five_gbk_lines() {
        let root = test_root("info");
        let path = root.join("Info.txt");
        let mut info = ThemeInfo::default();
        info.set(ThemeComponent::LoadScreen, "晨曦");

        write_info(&path, &info).unwrap();

        let bytes = fs::read(&path).unwrap();
        assert!(String::from_utf8(bytes.clone()).is_err());
        assert_eq!(read_info(&path).unwrap(), info);
        let decoded = GBK.decode(&bytes).0;
        assert_eq!(decoded.lines().count(), INFO_LINES);
        assert!(decoded.lines().all(|line| line.ends_with(' ')));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standalone_loadscreen_cleanup_preserves_non_jpg_and_nested_content() {
        let root = test_root("loadscreen-merge");
        let existing = root.join("existing");
        let prepared = root.join("prepared");
        fs::create_dir_all(existing.join("nested")).unwrap();
        fs::create_dir_all(&prepared).unwrap();
        fs::write(existing.join("old.jpg"), b"old").unwrap();
        fs::write(existing.join("OLD.JPG"), b"old-upper").unwrap();
        fs::write(existing.join("keep.ini"), b"keep").unwrap();
        fs::write(existing.join("nested/old.jpg"), b"nested").unwrap();

        copy_directory_contents(&existing, &prepared).unwrap();
        remove_top_level_jpegs(&prepared).unwrap();

        assert!(!prepared.join("old.jpg").exists());
        assert!(!prepared.join("OLD.JPG").exists());
        assert_eq!(fs::read(prepared.join("keep.ini")).unwrap(), b"keep");
        assert_eq!(
            fs::read(prepared.join("nested/old.jpg")).unwrap(),
            b"nested"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_component_inventory_uses_the_legacy_order() {
        let entries = vec![
            entry("SystemIconPack.ess"),
            entry("WallPaper.jpg"),
            entry("LoadScreen.els"),
        ];

        let components = theme_components(&entries).unwrap();

        assert_eq!(
            components.iter().map(|item| item.0).collect::<Vec<_>>(),
            vec![
                ThemeComponent::Wallpaper,
                ThemeComponent::LoadScreen,
                ThemeComponent::SystemIconPack,
            ]
        );
    }

    #[test]
    fn publish_failure_restores_every_replaced_destination() {
        let root = test_root("rollback");
        let edgeless = root.join("Edgeless");
        fs::create_dir_all(&edgeless).unwrap();
        let staging = StagingDirectory::new(&edgeless).unwrap();
        let first = edgeless.join("first.txt");
        let second = edgeless.join("second.txt");
        fs::write(&first, b"old-first").unwrap();
        fs::write(&second, b"old-second").unwrap();
        let staged_first = staging.path.join("first.txt");
        fs::write(&staged_first, b"new-first").unwrap();
        let missing = staging.path.join("missing.txt");
        let changes = vec![
            PublishChange {
                staged: Some(staged_first),
                destination: first.clone(),
            },
            PublishChange {
                staged: Some(missing),
                destination: second.clone(),
            },
        ];

        assert!(publish_plan(&edgeless, &staging, &changes).is_err());
        assert_eq!(fs::read(first).unwrap(), b"old-first");
        assert_eq!(fs::read(second).unwrap(), b"old-second");
        drop(staging);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_an_interrupted_replacement_before_the_next_store() {
        let root = test_root("recovery");
        let edgeless = root.join("Edgeless");
        let staging = edgeless.join(".eli-theme-store-interrupted");
        let backup = staging.join("backup/0");
        let destination = edgeless.join("Default/Info.txt");
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"partial").unwrap();
        fs::write(&backup, b"original").unwrap();
        let mut journal = File::create(staging.join("transaction.jsonl")).unwrap();
        write_journal(
            &mut journal,
            &JournalRecord::Change {
                destination: PathBuf::from("Default/Info.txt"),
                backup: PathBuf::from("backup/0"),
                had_original: true,
            },
        )
        .unwrap();
        drop(journal);

        recover_interrupted_transactions(&edgeless).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"original");
        assert!(!staging.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malicious_recovery_log_cannot_target_the_edgeless_root() {
        let root = test_root("unsafe-recovery");
        let edgeless = root.join("Edgeless");
        let staging = edgeless.join(".eli-theme-store-malicious");
        fs::create_dir_all(staging.join("backup")).unwrap();
        fs::write(edgeless.join("keep.txt"), b"keep").unwrap();
        let mut journal = File::create(staging.join("transaction.jsonl")).unwrap();
        write_journal(
            &mut journal,
            &JournalRecord::Change {
                destination: PathBuf::new(),
                backup: PathBuf::from("backup/0"),
                had_original: false,
            },
        )
        .unwrap();
        drop(journal);

        let error = recover_interrupted_transactions(&edgeless).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(edgeless.join("keep.txt")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    fn entry(path: &str) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 1,
            is_directory: false,
            encrypted: false,
            is_link: false,
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-store-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
