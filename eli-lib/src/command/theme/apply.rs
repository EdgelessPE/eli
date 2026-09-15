// `eli theme apply` 公开编排器。
//
// 流程为“识别 → 环境/能力检查 → 依赖探测 → 预检（无系统副作用）→ 获取主题
// 提交 named mutex → 按数据依赖提交组件并累计 RefreshRequest → 统一最小刷新 →
// ApplySummary”。所有系统副作用集中在提交与刷新阶段；预检只读解包/解码/校验。

#[path = "apply/mod.rs"]
mod details;

use crate::Ctx;
use crate::dependency::{RuntimeCapability, RuntimeEnvironment};
use std::io;
use std::path::Path;

use details::archive::{ETH_LIMITS, validate_listing};
use details::refresh::{RefreshPlan, RefreshRequest};
#[cfg(test)]
pub use details::test_support;
use details::transaction::StagingDir;
pub use details::{
    ApplySummary, ComponentOutcome, ComponentStatus, EisStats, ExecutedRefresh, ThemeComponent,
    ThemeType,
};
use details::{ThemeBackend, ThemePaths};

/// ELS 迁移警告的稳定消息（每个 ELS 组件只输出一次）。具体中英文措辞可由
/// CLI 本地化，但关键字由测试与端到端用例断言。
pub const ELS_WARNING: &str = "legacy LoadScreen.els is a startup resource and cannot affect the current PE session; skipped by `eli theme apply`; startup persistence is outside this command";

/// 识别外层主题包类型并确认输入是普通文件（在任何副作用前完成）。
pub fn identify_package(package: &Path) -> io::Result<ThemeType> {
    let metadata = std::fs::metadata(package).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to inspect theme package {}: {error}",
                package.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("theme input is not a regular file: {}", package.display()),
        ));
    }
    let Some(extension) = package.extension() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unsupported theme package: no extension on {}",
                package.display()
            ),
        ));
    };
    let Some(extension) = extension.to_str() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported theme package extension: {}", package.display()),
        ));
    };
    match extension.to_ascii_lowercase().as_str() {
        "eth" => Ok(ThemeType::Eth),
        "eis" => Ok(ThemeType::Eis),
        "ems" => Ok(ThemeType::Ems),
        "esc" => Ok(ThemeType::Esc),
        "ess" => Ok(ThemeType::Ess),
        "els" => Ok(ThemeType::Els),
        "jpg" => Ok(ThemeType::Jpg),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unsupported theme package extension .{extension}; expected eth, eis, ems, esc, ess, els or jpg"
            ),
        )),
    }
}

/// 公开入口：扩展名识别 → WindowsPE 环境 → EdgelessRuntime 防迁移门禁 →
/// 平台实现。`--bootdisk` 对本命令没有作用（本命令不修改启动盘）。
pub fn apply(ctx: &Ctx, package: &Path) -> io::Result<ApplySummary> {
    let kind = identify_package(package)?;
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)
        .map_err(|error| package_error(package, kind, "environment validation", error))?;
    ctx.dependencies()
        .require_capability(RuntimeCapability::EdgelessRuntime)
        .map_err(|error| package_error(package, kind, "environment validation", error))?;
    apply_on_supported_platform(ctx, package, kind)
}

fn package_error(package: &Path, kind: ThemeType, phase: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "theme package {} ({}) failed during {phase}: {error}",
            package.display(),
            kind.display_name()
        ),
    )
}

#[cfg(windows)]
fn apply_on_supported_platform(
    ctx: &Ctx,
    package: &Path,
    kind: ThemeType,
) -> io::Result<ApplySummary> {
    let backend = details::windows::WindowsThemeBackend::new(ctx)
        .map_err(|error| package_error(package, kind, "backend initialization", error))?;
    // 全量预检（无系统副作用）在锁外完成。
    let prepared = prepare_package(package, kind, &backend)
        .map_err(|error| package_error(package, kind, "precheck", error))?;
    let lock = details::windows::ThemeApplyLock::new()
        .map_err(|error| package_error(package, kind, "commit lock creation", error))?;
    let _guard = lock
        .acquire()
        .map_err(|error| package_error(package, kind, "commit lock acquisition", error))?;
    commit_prepared(&prepared, &backend)
        .map_err(|error| package_error(package, kind, "commit", error))
}

#[cfg(not(windows))]
fn apply_on_supported_platform(
    _ctx: &Ctx,
    _package: &Path,
    _kind: ThemeType,
) -> io::Result<ApplySummary> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "theme apply is only implemented for Windows PE",
    ))
}

/// 供平台无关单元测试使用的完整流程（不获取真实 named mutex）。
#[cfg(test)]
fn apply_with_backend(
    package: &Path,
    kind: ThemeType,
    backend: &dyn ThemeBackend,
) -> io::Result<ApplySummary> {
    let prepared = prepare_package(package, kind, backend)?;
    commit_prepared(&prepared, backend)
}

/// 已完成的预检主题（尚未产生任何系统副作用）。
///
/// `staging` 仅在需要解压/发布资源时创建；ELS 独立包与只含 ELS 的 `.eth`
/// 不创建 staging（SDD：ELS 不解析 7-Zip、不创建 staging、不打开归档）。
struct PreparedTheme {
    source: std::path::PathBuf,
    kind: ThemeType,
    paths: ThemePaths,
    staging: Option<StagingDir>,
    components: Vec<PreparedComponent>,
    warnings: Vec<String>,
}

enum PreparedComponent {
    Wallpaper(details::wallpaper::PreparedWallpaper),
    LoadScreen,
    MouseStyle(Box<details::ems::PreparedEms>),
    IconPack(Box<details::eis::PreparedEis>),
    StartIsBackConfig(details::esc::PreparedEsc),
    SystemIconPack(Box<details::ess::PreparedEss>),
}

fn prepare_package(
    package: &Path,
    kind: ThemeType,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedTheme> {
    match kind {
        ThemeType::Els => Ok(PreparedTheme {
            source: package.to_owned(),
            kind,
            paths: backend.theme_paths()?,
            staging: None,
            components: vec![PreparedComponent::LoadScreen],
            warnings: vec![ELS_WARNING.to_owned()],
        }),
        ThemeType::Jpg => {
            backend.require_pecmd()?;
            let paths = backend.theme_paths()?;
            let staging = StagingDir::create(&paths)?;
            let wallpaper = details::wallpaper::prepare_wallpaper(package, backend)?;
            Ok(PreparedTheme {
                source: package.to_owned(),
                kind,
                paths,
                staging: Some(staging),
                components: vec![PreparedComponent::Wallpaper(wallpaper)],
                warnings: vec![],
            })
        }
        ThemeType::Esc => {
            backend.require_pecmd()?;
            let paths = backend.theme_paths()?;
            let staging = StagingDir::create(&paths)?;
            let esc = details::esc::prepare_esc(package)?;
            Ok(PreparedTheme {
                source: package.to_owned(),
                kind,
                paths,
                staging: Some(staging),
                components: vec![PreparedComponent::StartIsBackConfig(esc)],
                warnings: vec![],
            })
        }
        ThemeType::Eis => {
            backend.require_seven_zip()?;
            let paths = backend.theme_paths()?;
            let staging = StagingDir::create(&paths)?;
            let icon_pack = details::eis::prepare_eis_from_archive(
                package,
                &staging.join("eis"),
                &paths,
                backend,
            )?;
            Ok(PreparedTheme {
                source: package.to_owned(),
                kind,
                paths,
                staging: Some(staging),
                components: vec![PreparedComponent::IconPack(Box::new(icon_pack))],
                warnings: vec![],
            })
        }
        ThemeType::Ems => {
            backend.require_seven_zip()?;
            let paths = backend.theme_paths()?;
            let staging = StagingDir::create(&paths)?;
            let mouse_style = details::ems::prepare_ems_from_archive(
                package,
                &staging.join("ems"),
                &paths,
                backend,
            )?;
            Ok(PreparedTheme {
                source: package.to_owned(),
                kind,
                paths,
                staging: Some(staging),
                components: vec![PreparedComponent::MouseStyle(Box::new(mouse_style))],
                warnings: vec![],
            })
        }
        ThemeType::Ess => {
            backend.require_seven_zip()?;
            let paths = backend.theme_paths()?;
            let staging = StagingDir::create(&paths)?;
            let system_icon_pack =
                details::ess::prepare_ess_from_archive(package, &staging.join("ess"), backend)?;
            Ok(PreparedTheme {
                source: package.to_owned(),
                kind,
                paths,
                staging: Some(staging),
                components: vec![PreparedComponent::SystemIconPack(Box::new(
                    system_icon_pack,
                ))],
                warnings: vec![],
            })
        }
        ThemeType::Eth => prepare_eth_package(package, backend),
    }
}

/// `.eth` 组合主题：列出根目录 → 识别规范组件 → PECMD 依赖判断 → 白名单解压 →
/// 全部可应用组件的独立预检（ELS 只记录跳过）。
fn prepare_eth_package(package: &Path, backend: &dyn ThemeBackend) -> io::Result<PreparedTheme> {
    backend.require_seven_zip()?;
    let paths = backend.theme_paths()?;
    let entries = backend.list_archive(package)?;
    validate_listing(&entries, &ETH_LIMITS)?;

    // 根条目按规范组件名（ASCII 大小写不敏感）识别；同名大小写变体拒绝。
    let mut found = Vec::new();
    for entry in &entries {
        if entry.path.contains('/') {
            continue;
        }
        let name = entry.path.to_ascii_lowercase();
        let component = match name.as_str() {
            "wallpaper.jpg" => Some(ThemeComponent::Wallpaper),
            "loadscreen.els" => Some(ThemeComponent::LoadScreen),
            "iconpack.eis" => Some(ThemeComponent::IconPack),
            "mousestyle.ems" => Some(ThemeComponent::MouseStyle),
            "startisbackconfig.esc" => Some(ThemeComponent::StartIsBackConfig),
            "systemiconpack.ess" => Some(ThemeComponent::SystemIconPack),
            // Intro 只属于“打开主题包时的交互控制面板”，不是应用载荷。
            "intro.txt" | "intro.wcs" | "intro" => continue,
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
                    "theme package contains multiple case variants of the component {}: {}",
                    component.display_name(),
                    entry.path
                ),
            ));
        }
        found.push((component, entry.path.clone()));
    }

    // 未知根条目打印警告并忽略。
    let mut warnings = Vec::new();
    let mut unknown_roots = Vec::new();
    for entry in &entries {
        let root_name = entry.path.trim_end_matches('/');
        if root_name.contains('/') {
            continue;
        }
        let name = root_name.to_ascii_lowercase();
        if found.iter().any(|(_, path)| path == &entry.path)
            || matches!(name.as_str(), "intro.txt" | "intro.wcs" | "intro")
        {
            continue;
        }
        unknown_roots.push(entry.path.clone());
    }
    for unknown in &unknown_roots {
        warnings.push(format!(
            "ignored an unknown root entry in the theme package: {unknown}"
        ));
    }

    // 规范顺序：壁纸、ELS、EIS、EMS、ESC、ESS。
    let mut components: Vec<(ThemeComponent, String)> = Vec::with_capacity(found.len());
    for component in [
        ThemeComponent::Wallpaper,
        ThemeComponent::LoadScreen,
        ThemeComponent::IconPack,
        ThemeComponent::MouseStyle,
        ThemeComponent::StartIsBackConfig,
        ThemeComponent::SystemIconPack,
    ] {
        if let Some((_, path)) = found.iter().find(|(existing, _)| *existing == component) {
            components.push((component, path.clone()));
        }
    }
    let needs_pecmd = components.iter().any(|(component, _)| {
        matches!(
            component,
            ThemeComponent::Wallpaper | ThemeComponent::StartIsBackConfig
        )
    });
    // 依赖探测在创建 staging 前完成。
    let has_els_only = components.len() == 1 && components[0].0 == ThemeComponent::LoadScreen;
    if !has_els_only && needs_pecmd {
        backend.require_pecmd()?;
    }
    if has_els_only {
        warnings.push(ELS_WARNING.to_owned());
        return Ok(PreparedTheme {
            source: package.to_owned(),
            kind: ThemeType::Eth,
            paths,
            staging: None,
            components: vec![PreparedComponent::LoadScreen],
            warnings,
        });
    }

    let staging = if components.is_empty() {
        None
    } else {
        let staging = StagingDir::create(&paths)?;
        let whitelist = components
            .iter()
            .filter(|(component, _)| *component != ThemeComponent::LoadScreen)
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        backend.extract_archive_entries(package, staging.path(), &whitelist)?;
        details::archive::verify_extraction(staging.path())?;
        Some(staging)
    };

    // 嵌套组件独立预检。
    let mut prepared_components = Vec::new();
    for (component, path) in &components {
        if *component == ThemeComponent::LoadScreen {
            warnings.push(ELS_WARNING.to_owned());
            prepared_components.push(PreparedComponent::LoadScreen);
            continue;
        }
        let staged = staging
            .as_ref()
            .expect("a non-ELS component always has staging")
            .join(path);
        match component {
            ThemeComponent::LoadScreen => unreachable!("LoadScreen is handled without extraction"),
            ThemeComponent::Wallpaper => {
                let wallpaper = details::wallpaper::prepare_wallpaper(&staged, backend)?;
                prepared_components.push(PreparedComponent::Wallpaper(wallpaper));
            }
            ThemeComponent::IconPack => {
                let icon_pack = details::eis::prepare_eis_from_archive(
                    &staged,
                    &staging
                        .as_ref()
                        .expect("a non-ELS component always has staging")
                        .join("eis"),
                    &paths,
                    backend,
                )?;
                prepared_components.push(PreparedComponent::IconPack(Box::new(icon_pack)));
            }
            ThemeComponent::MouseStyle => {
                let mouse_style = details::ems::prepare_ems_from_archive(
                    &staged,
                    &staging
                        .as_ref()
                        .expect("a non-ELS component always has staging")
                        .join("ems"),
                    &paths,
                    backend,
                )?;
                prepared_components.push(PreparedComponent::MouseStyle(Box::new(mouse_style)));
            }
            ThemeComponent::StartIsBackConfig => {
                let esc = details::esc::prepare_esc(&staged)?;
                prepared_components.push(PreparedComponent::StartIsBackConfig(esc));
            }
            ThemeComponent::SystemIconPack => {
                let system_icon_pack = details::ess::prepare_ess_from_archive(
                    &staged,
                    &staging
                        .as_ref()
                        .expect("a non-ELS component always has staging")
                        .join("ess"),
                    backend,
                )?;
                prepared_components.push(PreparedComponent::SystemIconPack(Box::new(
                    system_icon_pack,
                )));
            }
        }
    }

    Ok(PreparedTheme {
        source: package.to_owned(),
        kind: ThemeType::Eth,
        paths,
        staging,
        components: prepared_components,
        warnings,
    })
}

/// 提交阶段：在 named mutex 内按数据依赖提交组件，累计刷新请求，
/// 最后由统一 RefreshPlan 以最小刷新集合执行。
fn commit_prepared(
    prepared: &PreparedTheme,
    backend: &dyn ThemeBackend,
) -> io::Result<ApplySummary> {
    let mut summary = ApplySummary {
        source: prepared.source.clone(),
        outer: prepared.kind,
        components: Vec::new(),
        warnings: prepared.warnings.clone(),
        eis: details::EisStats::default(),
        refresh: ExecutedRefresh::default(),
    };
    let mut refresh_plan = RefreshPlan::default();
    let mut cursors_refreshed = false;

    // ESS 需要在统一刷新阶段（Explorer 停止期间）执行双 DLL 替换。
    let mut pending_ess: Option<usize> = None;

    for component in &prepared.components {
        let outcome = match component {
            PreparedComponent::LoadScreen => ComponentOutcome {
                component: ThemeComponent::LoadScreen,
                status: ComponentStatus::Skipped,
            },
            PreparedComponent::Wallpaper(wallpaper) => {
                match details::wallpaper::commit_wallpaper(wallpaper, backend, &prepared.paths) {
                    Ok(()) => ComponentOutcome {
                        component: ThemeComponent::Wallpaper,
                        status: ComponentStatus::Applied,
                    },
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::Wallpaper,
                        status: ComponentStatus::Failed(error.to_string()),
                    },
                }
            }
            PreparedComponent::MouseStyle(mouse_style) => {
                match details::ems::commit_ems(mouse_style, backend) {
                    Ok(warnings) => {
                        cursors_refreshed = true;
                        ComponentOutcome {
                            component: ThemeComponent::MouseStyle,
                            status: if warnings.is_empty() {
                                ComponentStatus::Applied
                            } else {
                                ComponentStatus::AppliedWithWarnings(warnings)
                            },
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::MouseStyle,
                        status: ComponentStatus::Failed(error.to_string()),
                    },
                }
            }
            PreparedComponent::IconPack(icon_pack) => {
                match details::eis::commit_eis(
                    icon_pack,
                    backend,
                    &prepared.paths,
                    &mut refresh_plan,
                ) {
                    Ok(stats) => {
                        summary.eis = stats;
                        if stats.updated == 0 && stats.failed > 0 {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::Failed(format!(
                                    "every existing shortcut target failed to modify ({} failed)",
                                    stats.failed
                                )),
                            }
                        } else if stats.failed > 0 {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::AppliedWithWarnings(vec![format!(
                                    "{} shortcut link(s) failed to modify; already published icons remain usable",
                                    stats.failed
                                )]),
                            }
                        } else {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::Applied,
                            }
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::IconPack,
                        status: ComponentStatus::Failed(format!(
                            "icon resources failed to publish: {error}"
                        )),
                    },
                }
            }
            PreparedComponent::StartIsBackConfig(esc) => {
                match details::esc::commit_esc(esc, backend, &mut refresh_plan) {
                    Ok(()) => ComponentOutcome {
                        component: ThemeComponent::StartIsBackConfig,
                        status: ComponentStatus::Applied,
                    },
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::StartIsBackConfig,
                        status: ComponentStatus::Failed(error.to_string()),
                    },
                }
            }
            PreparedComponent::SystemIconPack(system_icon_pack) => {
                let transaction_dir =
                    prepared
                        .staging
                        .as_ref()
                        .map(StagingDir::path)
                        .ok_or_else(|| {
                            io::Error::other(
                                "ESS component requires a staging directory, but none was prepared",
                            )
                        })?;
                match system_icon_pack.commit_snapshot(transaction_dir, &prepared.paths) {
                    Ok(commit) => {
                        let index = summary.components.len();
                        pending_ess = Some(index);
                        refresh_plan.request(RefreshRequest::IconCacheInvalidate);
                        refresh_plan.request(RefreshRequest::SystemIconReplacement(commit));
                        ComponentOutcome {
                            component: ThemeComponent::SystemIconPack,
                            status: ComponentStatus::Applied,
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::SystemIconPack,
                        status: ComponentStatus::Failed(format!(
                            "failed to snapshot the system icon DLLs: {error}"
                        )),
                    },
                }
            }
        };
        summary.components.push(outcome);
    }

    // 统一最小刷新。
    let mut ess_error: Option<String> = None;
    let (executed, refresh_result) =
        refresh_plan.execute_minimal(backend, &prepared.paths, &mut |commit| match commit
            .replace(backend)
        {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = error.to_string();
                ess_error = Some(message.clone());
                Err(io::Error::new(error.kind(), message))
            }
        });
    summary.refresh = executed;
    summary.refresh.cursors_refreshed |= cursors_refreshed;

    // 汇总 ESS 结果与整体刷新失败。
    let mut refresh_failure: Option<String> = None;
    if let Err(error) = refresh_result {
        refresh_failure = Some(error.to_string());
    }
    if let Some(index) = pending_ess {
        let failure_attr = match (ess_error, refresh_failure) {
            (Some(ess_error), Some(refresh_error)) => Some(format!(
                "system icon DLL replacement failed and was rolled back: {ess_error}; the refresh phase also failed: {refresh_error}"
            )),
            (Some(ess_error), None) => Some(format!(
                "system icon DLL replacement failed and was rolled back: {ess_error}"
            )),
            (None, Some(refresh_error)) => Some(format!(
                "system icon apply refresh phase failed: {refresh_error}"
            )),
            (None, None) => None,
        };
        if let Some(message) = failure_attr {
            summary.components[index].status = ComponentStatus::Failed(message);
        }
    } else if let Some(refresh_error) = refresh_failure {
        // 没有 ESS 时，刷新阶段失败归属到请求 Explorer 重启的组件（ESC），
        // 以“部分成功错误”标记（资源已提交但 Shell 恢复失败）。
        let message =
            format!("theme resources were committed but Explorer recovery failed: {refresh_error}");
        for outcome in summary.components.iter_mut().rev() {
            if outcome.component != ThemeComponent::StartIsBackConfig {
                continue;
            }
            match &outcome.status {
                ComponentStatus::Applied => {
                    outcome.status = ComponentStatus::Failed(message.clone());
                }
                ComponentStatus::AppliedWithWarnings(_) => {
                    let mut warnings =
                        match std::mem::replace(&mut outcome.status, ComponentStatus::Applied) {
                            ComponentStatus::AppliedWithWarnings(warnings) => warnings,
                            _ => unreachable!(),
                        };
                    warnings.push(message.clone());
                    outcome.status = ComponentStatus::Failed(warnings.join("; "));
                }
                _ => {}
            }
            break;
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::test_support::{FakeArchive, FakeBackend};
    use details::archive::ArchiveEntry;
    use details::refresh::{RefreshPlan, RefreshRequest};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eli-theme-apply-{}-{}",
            std::process::id(),
            details::transaction::unique_transaction_id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn entry(path: &str, is_directory: bool) -> ArchiveEntry {
        ArchiveEntry {
            path: path.to_owned(),
            size: 0,
            is_directory,
            encrypted: false,
            is_link: false,
        }
    }

    fn register_archive(
        backend: &FakeBackend,
        key: PathBuf,
        entries: Vec<ArchiveEntry>,
        files: &[(&str, &[u8])],
    ) {
        let files = files
            .iter()
            .map(|(name, bytes)| (name.to_string(), bytes.to_vec()))
            .collect::<HashMap<_, _>>();
        let archive = FakeArchive { entries, files };
        let mut state = backend.state.lock().unwrap();
        let canonical = std::fs::canonicalize(&key).unwrap_or_else(|_| key.clone());
        state.archives.insert(canonical, archive);
    }

    fn register_standalone(backend: &FakeBackend, package: &Path, files: &[(&str, &[u8])]) {
        let entries = files
            .iter()
            .map(|(name, _)| entry(name, name.ends_with('/')))
            .collect();
        register_archive(backend, package.to_owned(), entries, files);
    }

    // 构造一个最小可用 PE 文件（供 ESS 的 imageres.dll/imagesp1.dll 预检）。
    fn pe_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x60];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(b"PE\0\0");
        let machine: u16 = if cfg!(target_arch = "x86_64") {
            0x8664
        } else {
            0x14c
        };
        bytes[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
        bytes[0x56..0x58].copy_from_slice(&0x2000u16.to_le_bytes());
        bytes
    }

    #[test]
    fn routes_all_documented_extensions_case_insensitively() {
        let root = test_root();
        for (extension, expected) in [
            ("eth", ThemeType::Eth),
            ("EIS", ThemeType::Eis),
            ("Ems", ThemeType::Ems),
            ("eSC", ThemeType::Esc),
            ("ess", ThemeType::Ess),
            ("ELS", ThemeType::Els),
            ("JPG", ThemeType::Jpg),
        ] {
            let package = root.join(format!("sample.{extension}"));
            std::fs::write(&package, "data").unwrap();
            assert_eq!(identify_package(&package).unwrap(), expected);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_unknown_extensions_directories_and_missing_extension() {
        let root = test_root();
        let png = root.join("sample.png");
        std::fs::write(&png, "data").unwrap();
        assert_eq!(
            identify_package(&png).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        let directory = root.join("folder.eth");
        std::fs::create_dir(&directory).unwrap();
        assert_eq!(
            identify_package(&directory).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        let no_extension = root.join("sample");
        std::fs::write(&no_extension, "data").unwrap();
        assert_eq!(
            identify_package(&no_extension).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn standalone_els_skips_without_opening_the_archive() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("loadscreen.els");
        std::fs::write(&package, "ignored content").unwrap();
        register_standalone(&backend, &package, &[]);

        let summary = apply_with_backend(&package, ThemeType::Els, &backend).unwrap();

        assert!(summary.is_success());
        assert_eq!(summary.applied(), 0);
        assert_eq!(summary.skipped(), 1);
        assert_eq!(summary.failed(), 0);
        assert_eq!(summary.warnings.len(), 1);
        assert!(summary.warnings[0].contains("LoadScreen.els"));

        // 不解析 7-Zip、不创建 staging、不打开归档。
        let state = backend.state.lock().unwrap();
        assert!(!state.log.iter().any(|event| event == "list_archive"));
        assert!(
            !state
                .log
                .iter()
                .any(|event| event == "extract_archive_entries")
        );
        assert!(!backend.paths.staging_root.exists());
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eth_with_only_els_returns_zero_applied_one_skipped() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("theme.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![entry("LoadScreen.els", false)],
            &[("LoadScreen.els", b"ignored")],
        );

        let summary = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap();

        assert!(summary.is_success());
        assert_eq!(summary.applied(), 0);
        assert_eq!(summary.skipped(), 1);
        assert!(!backend.paths.staging_root.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eth_with_other_components_does_not_extract_nested_els() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("theme.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![
                entry("WallPaper.jpg", false),
                entry("LoadScreen.els", false),
            ],
            &[
                ("WallPaper.jpg", b"\xff\xd8\xff\xe0 fake jpeg"),
                ("LoadScreen.els", b"must not be extracted"),
            ],
        );

        let summary = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap();

        assert_eq!(summary.applied(), 1);
        assert_eq!(summary.skipped(), 1);
        let state = backend.state.lock().unwrap();
        assert_eq!(state.extraction_whitelists.len(), 1);
        assert_eq!(
            state.extraction_whitelists[0],
            vec!["WallPaper.jpg".to_owned()]
        );
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn empty_eth_is_a_successful_no_op_with_unknown_entry_warnings() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("empty.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![
                entry("Intro.txt", false),
                entry("Intro/", true),
                entry("Readme.txt", false),
                entry("Extras/", true),
                entry("Extras/note.txt", false),
            ],
            &[
                ("Intro.txt", b"intro"),
                ("Readme.txt", b"readme"),
                ("Extras/note.txt", b"note"),
            ],
        );

        let summary = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap();

        assert!(summary.is_success());
        assert_eq!(summary.applied(), 0);
        assert_eq!(summary.skipped(), 0);
        assert!(
            summary
                .warnings
                .iter()
                .any(|warning| warning.contains("Readme.txt"))
        );
        assert!(
            summary
                .warnings
                .iter()
                .any(|warning| warning.contains("Extras/"))
        );
        assert!(
            !summary
                .warnings
                .iter()
                .any(|warning| warning.contains("Extras/note.txt"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eth_rejects_case_variant_duplicate_components() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("dup.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![entry("WallPaper.jpg", false), entry("wallpaper.jpg", false)],
            &[("WallPaper.jpg", b"jpg"), ("wallpaper.jpg", b"jpg")],
        );

        let error = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap_err();

        assert!(error.to_string().contains("collision"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eth_rejects_path_traversal_entries_before_any_side_effect() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("evil.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![entry("../escape.txt", false)],
            &[("../escape.txt", b"evil")],
        );

        let error = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap_err();

        assert!(error.to_string().contains("unsafe archive entry path"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    // 全部预检完成后才允许进入提交后端。
    #[test]
    fn all_prechecks_complete_before_any_commit_call() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("full.eth");
        let wallpaper = root.join("WallPaper.jpg");
        std::fs::write(&package, "eth data").unwrap();
        std::fs::write(&wallpaper, b"\xff\xd8\xff\xe0 fake jpeg").unwrap();

        let entries = vec![
            entry("WallPaper.jpg", false),
            entry("LoadScreen.els", false),
            entry("IconPack.eis", false),
            entry("MouseStyle.ems", false),
            entry("StartIsBackConfig.esc", false),
            entry("SystemIconPack.ess", false),
        ];
        register_archive(
            &backend,
            package.clone(),
            entries,
            &[
                ("WallPaper.jpg", b"\xff\xd8\xff\xe0 fake jpeg"),
                ("LoadScreen.els", b"els"),
                ("IconPack.eis", b"eis"),
                ("MouseStyle.ems", b"ems"),
                ("StartIsBackConfig.esc", b"pecmd script"),
                ("SystemIconPack.ess", b"ess"),
            ],
        );
        register_standalone(&backend, &wallpaper, &[]);

        // 嵌套资源包按文件名匹配。
        let eis = FakeArchive {
            entries: vec![entry("shortcut/", true), entry("shortcut/App.ico", false)],
            files: [("shortcut/App.ico".to_owned(), vec![1, 2, 3])]
                .into_iter()
                .collect(),
        };
        let ems_entries: Vec<ArchiveEntry> = (0..15)
            .map(|slot| {
                let base = [
                    "aero_arrow",
                    "aero_helpsel",
                    "aero_working",
                    "aero_busy",
                    "aero_cross",
                    "aero_beam",
                    "aero_pen",
                    "aero_unavail",
                    "aero_ns",
                    "aero_ew",
                    "aero_nwse",
                    "aero_nesw",
                    "aero_move",
                    "aero_up",
                    "aero_link",
                ][slot];
                entry(&format!("{base}.ani"), false)
            })
            .collect();
        let ems_files = ems_entries
            .iter()
            .map(|e| (e.path.clone(), vec![0u8]))
            .collect();
        let ess = FakeArchive {
            entries: vec![entry("imageres.dll", false), entry("imagesp1.dll", false)],
            files: [
                ("imageres.dll".to_owned(), pe_bytes()),
                ("imagesp1.dll".to_owned(), pe_bytes()),
            ]
            .into_iter()
            .collect(),
        };
        let mut state = backend.state.lock().unwrap();
        state.nested_by_name.insert("ICONPACK.EIS".to_owned(), eis);
        state.nested_by_name.insert(
            "MOUSESTYLE.EMS".to_owned(),
            FakeArchive {
                entries: ems_entries,
                files: ems_files,
            },
        );
        state
            .nested_by_name
            .insert("SYSTEMICONPACK.ESS".to_owned(), ess);
        drop(state);

        std::fs::create_dir_all(backend.paths.system_root.join("System32")).unwrap();

        let summary = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap();

        assert_eq!(summary.applied(), 5);
        assert_eq!(summary.skipped(), 1);
        assert!(summary.is_success());

        // 顺序断言：所有预检（列出/解压/解码/光标验证）在第一个提交调用之前。
        let log = backend.state.lock().unwrap().log.clone();
        let prepare_markers = [
            "list_archive",
            "extract_archive_entries",
            "decode_jpeg",
            "decode_icon",
            "decode_plain_image",
            "validate_cursor_file",
        ];
        let commit_markers = [
            "apply_wallpaper",
            "publish_cursor_directory",
            "snapshot_cursors",
            "write_cursor_slots",
            "write_cursor_scheme",
            "write_cursor_default_scheme",
            "modify_shortcut_icon",
            "execute_esc",
            "verify_shell_context",
            "refresh_cursors",
        ];
        let last_prepare = log
            .iter()
            .enumerate()
            .filter(|(_, event)| prepare_markers.contains(&event.as_str()))
            .map(|(index, _)| index)
            .max()
            .unwrap();
        let first_commit = log
            .iter()
            .enumerate()
            .filter(|(_, event)| commit_markers.contains(&event.as_str()))
            .map(|(index, _)| index)
            .min()
            .unwrap();
        assert!(
            last_prepare < first_commit,
            "prechecks must finish before commits"
        );

        // 提交阶段并发数始终为 1。
        let state = backend.state.lock().unwrap();
        assert_eq!(state.max_active, 1);
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_component_does_not_block_independent_following_components() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let package = root.join("partial.eth");
        std::fs::write(&package, "eth data").unwrap();
        register_archive(
            &backend,
            package.clone(),
            vec![
                entry("WallPaper.jpg", false),
                entry("MouseStyle.ems", false),
                entry("StartIsBackConfig.esc", false),
            ],
            &[
                ("WallPaper.jpg", b"\xff\xd8\xff\xe0 fake jpeg"),
                ("MouseStyle.ems", b"ems"),
                ("StartIsBackConfig.esc", b"pecmd script"),
            ],
        );
        let mut state = backend.state.lock().unwrap();
        state.fail_cursor_write = true;
        let ems_entries: Vec<ArchiveEntry> = (0..15)
            .map(|slot| {
                let base = [
                    "aero_arrow",
                    "aero_helpsel",
                    "aero_working",
                    "aero_busy",
                    "aero_cross",
                    "aero_beam",
                    "aero_pen",
                    "aero_unavail",
                    "aero_ns",
                    "aero_ew",
                    "aero_nwse",
                    "aero_nesw",
                    "aero_move",
                    "aero_up",
                    "aero_link",
                ][slot];
                entry(&format!("{base}.ani"), false)
            })
            .collect();
        state.nested_by_name.insert(
            "MOUSESTYLE.EMS".to_owned(),
            FakeArchive {
                entries: ems_entries,
                files: HashMap::new(),
            },
        );
        drop(state);

        let summary = apply_with_backend(&package, ThemeType::Eth, &backend).unwrap();

        assert!(!summary.is_success());
        assert_eq!(summary.failed(), 1);
        assert_eq!(summary.applied(), 2);
        assert!(
            summary
                .components
                .iter()
                .any(|outcome| outcome.component == ThemeComponent::MouseStyle
                    && matches!(outcome.status, ComponentStatus::Failed(_)))
        );
        assert!(summary.components.iter().any(|outcome| outcome.component
            == ThemeComponent::StartIsBackConfig
            && matches!(outcome.status, ComponentStatus::Applied)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eis_updates_matching_desktop_links_and_counts_unmatched_icons() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let desktop = backend.paths.desktop_roots[0].clone();
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("App.lnk"), "link").unwrap();
        let package = root.join("icons.eis");
        std::fs::write(&package, "eis data").unwrap();
        register_standalone(
            &backend,
            &package,
            &[
                ("shortcut/", b""),
                ("shortcut/App.ico", b"\0\0\0\0 fake ico"),
                ("shortcut/Ghost.ico", b"\0\0\0\0 fake ico"),
                ("Icon/logo.png", b"fake png"),
            ],
        );

        let summary = apply_with_backend(&package, ThemeType::Eis, &backend).unwrap();

        assert!(summary.is_success());
        assert_eq!(summary.eis.updated, 1);
        assert_eq!(summary.eis.not_found, 1);
        assert_eq!(summary.eis.failed, 0);
        // 图标发布到 Users/Icon；App.lnk 只修改了图标位置。
        let state = backend.state.lock().unwrap();
        assert_eq!(state.modified_links.len(), 1);
        assert!(backend.paths.icon_root.join("shortcut/App.ico").exists());
        assert!(backend.paths.icon_root.join("Icon/logo.png").exists());
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn eis_follows_per_link_best_effort_on_failed_links() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let desktop = backend.paths.desktop_roots[0].clone();
        std::fs::create_dir_all(&desktop).unwrap();
        std::fs::write(desktop.join("App1.lnk"), "link").unwrap();
        let app2 = desktop.join("App2.lnk");
        std::fs::write(&app2, "link").unwrap();
        let package = root.join("icons.eis");
        std::fs::write(&package, "eis data").unwrap();
        register_standalone(
            &backend,
            &package,
            &[
                ("shortcut/App1.ico", b"fake ico 1"),
                ("shortcut/App2.ico", b"fake ico 2"),
            ],
        );
        backend
            .state
            .lock()
            .unwrap()
            .link_modify_failures
            .push(app2.clone());

        let summary = apply_with_backend(&package, ThemeType::Eis, &backend).unwrap();

        assert_eq!(summary.eis.updated, 1);
        assert_eq!(summary.eis.failed, 1);
        let outcome = summary
            .components
            .iter()
            .find(|outcome| outcome.component == ThemeComponent::IconPack)
            .unwrap();
        assert!(matches!(
            outcome.status,
            ComponentStatus::AppliedWithWarnings(_)
        ));
        let state = backend.state.lock().unwrap();
        assert_eq!(state.modified_links.len(), 1);
        assert_eq!(state.modified_links[0].0, desktop.join("App1.lnk"));
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    // RefreshPlan 合并语义（SDD 16 / 19.5）。
    fn ess_commit_for(root: &Path) -> details::ess::EssCommit {
        let system32 = backend_paths(root).system_root.join("System32");
        std::fs::create_dir_all(&system32).unwrap();
        std::fs::write(system32.join("imageres.dll"), b"old-imageres").unwrap();
        std::fs::write(system32.join("imagesp1.dll"), b"old-imagesp1").unwrap();
        let staging = root.join("ess-staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("imageres.dll"), pe_bytes()).unwrap();
        std::fs::write(staging.join("imagesp1.dll"), pe_bytes()).unwrap();
        let prepared = details::ess::PreparedEss {
            imageres_source: staging.join("imageres.dll"),
            imagesp1_source: staging.join("imagesp1.dll"),
        };
        prepared
            .commit_snapshot(&root.join("ess-backup"), &backend_paths(root))
            .unwrap()
    }

    fn backend_paths(root: &Path) -> ThemePaths {
        ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("staging"),
            wallpaper_dir: root.join("wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: vec![],
            icon_cache_dir: root.join("Cache"),
        }
    }

    #[test]
    fn plan_merges_restarts_and_suppresses_shortcut_notifications() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        backend.state.lock().unwrap().shell_running = true;
        let mut plan = RefreshPlan::default();
        plan.request(RefreshRequest::ExplorerRestart);
        plan.request(RefreshRequest::ExplorerRestart);
        plan.request(RefreshRequest::IconCacheInvalidate);
        plan.request(RefreshRequest::ShortcutNotify(root.join("App1.lnk")));
        plan.request(RefreshRequest::ShortcutNotify(root.join("App2.lnk")));
        plan.request(RefreshRequest::SystemIconReplacement(ess_commit_for(&root)));

        let (executed, refresh_result) =
            plan.execute_minimal(&backend, &backend.paths.clone(), &mut |_| Ok(()));

        assert!(refresh_result.is_ok());
        assert!(executed.explorer_restarted);
        assert!(executed.icon_cache_invalidated);
        assert_eq!(executed.shortcut_notified, 0);
        let state = backend.state.lock().unwrap();
        assert_eq!(state.shell_events, vec!["stop", "start"]);
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_without_restart_sends_targeted_shortcut_notifications() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        let mut plan = RefreshPlan::default();
        plan.request(RefreshRequest::ShortcutNotify(root.join("App1.lnk")));
        plan.request(RefreshRequest::ShortcutNotify(root.join("App2.lnk")));

        let (executed, refresh_result) =
            plan.execute_minimal(&backend, &backend.paths.clone(), &mut |_| unreachable!());

        assert!(refresh_result.is_ok());
        assert!(!executed.explorer_restarted);
        assert_eq!(executed.shortcut_notified, 2);
        assert!(!executed.icon_cache_invalidated);
        let state = backend.state.lock().unwrap();
        assert!(state.shell_events.is_empty());
        drop(state);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_keeps_cursor_reload_independent_of_explorer_restart() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        backend.state.lock().unwrap().shell_running = true;
        let mut plan = RefreshPlan::default();
        plan.request(RefreshRequest::ExplorerRestart);
        plan.request(RefreshRequest::CursorReload);

        let (executed, refresh_result) =
            plan.execute_minimal(&backend, &backend.paths.clone(), &mut |_| unreachable!());

        assert!(refresh_result.is_ok());
        assert!(executed.explorer_restarted);
        assert!(executed.cursors_refreshed);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ess_replacement_failure_restores_dlls_and_recovers_shell() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        backend.state.lock().unwrap().shell_running = true;
        let mut plan = RefreshPlan::default();
        let commit = ess_commit_for(&root);
        plan.request(RefreshRequest::IconCacheInvalidate);
        // 模拟第二个 DLL 缺失造成的替换失败。
        std::fs::remove_file(commit.imagesp1_source()).unwrap();
        plan.request(RefreshRequest::SystemIconReplacement(commit));

        let (executed, refresh_result) =
            plan.execute_minimal(&backend, &backend.paths.clone(), &mut |commit| {
                commit.replace(&backend)
            });

        assert!(refresh_result.is_err());
        assert!(executed.explorer_restarted);
        let system32 = backend.paths.system_root.join("System32");
        assert_eq!(
            std::fs::read(system32.join("imageres.dll")).unwrap(),
            b"old-imageres"
        );
        assert_eq!(
            std::fs::read(system32.join("imagesp1.dll")).unwrap(),
            b"old-imagesp1"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn edgeless_runtime_gate_is_required_before_any_side_effect() {
        let ctx = Ctx::new(None);
        let error = ctx
            .dependencies()
            .require_capability(RuntimeCapability::EdgelessRuntime)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}
