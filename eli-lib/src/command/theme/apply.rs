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

#[cfg(any(windows, test))]
use details::archive::{ETH_LIMITS, validate_listing};
#[cfg(any(windows, test))]
use details::refresh::{RefreshPlan, RefreshRequest};
#[cfg(test)]
pub use details::test_support;
#[cfg(any(windows, test))]
use details::transaction::StagingDir;
pub use details::{
    ApplySummary, ComponentOutcome, ComponentStatus, EisStats, ExecutedRefresh, ThemeComponent,
    ThemeType,
};
#[cfg(any(windows, test))]
use details::{ThemeBackend, ThemePaths};

/// ELS 迁移警告的稳定消息（每个 ELS 组件只输出一次）。具体中英文措辞可由
/// CLI 本地化，但关键字由测试与端到端用例断言。
#[cfg(any(windows, test))]
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
            "theme package {} (outer type {}, component {}) failed during {phase}: {error}",
            package.display(),
            kind.display_name(),
            outer_component_name(kind),
        ),
    )
}

fn outer_component_name(kind: ThemeType) -> &'static str {
    match kind {
        ThemeType::Eth => "combined theme plan",
        ThemeType::Eis => ThemeComponent::IconPack.display_name(),
        ThemeType::Ems => ThemeComponent::MouseStyle.display_name(),
        ThemeType::Esc => ThemeComponent::StartIsBackConfig.display_name(),
        ThemeType::Ess => ThemeComponent::SystemIconPack.display_name(),
        ThemeType::Els => ThemeComponent::LoadScreen.display_name(),
        ThemeType::Jpg => ThemeComponent::Wallpaper.display_name(),
    }
}

#[cfg(any(windows, test))]
fn component_error(component: ThemeComponent, phase: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "component {} failed during {phase}: {error}",
            component.display_name()
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
#[cfg(any(windows, test))]
struct PreparedTheme {
    source: std::path::PathBuf,
    kind: ThemeType,
    /// 只有会真正提交资源的主题才解析运行时写入路径；纯 ELS 跳过不依赖桌面等路径。
    paths: Option<ThemePaths>,
    staging: Option<StagingDir>,
    components: Vec<PreparedComponent>,
    warnings: Vec<String>,
}

#[cfg(any(windows, test))]
enum PreparedComponent {
    Wallpaper(details::wallpaper::PreparedWallpaper),
    LoadScreen,
    MouseStyle(Box<details::ems::PreparedEms>),
    IconPack(Box<details::eis::PreparedEis>),
    StartIsBackConfig(details::esc::PreparedEsc),
    SystemIconPack(Box<details::ess::PreparedEss>),
}

#[cfg(any(windows, test))]
fn prepare_package(
    package: &Path,
    kind: ThemeType,
    backend: &dyn ThemeBackend,
) -> io::Result<PreparedTheme> {
    match kind {
        ThemeType::Els => Ok(PreparedTheme {
            source: package.to_owned(),
            kind,
            paths: None,
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
                paths: Some(paths),
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
                paths: Some(paths),
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
                paths: Some(paths),
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
                paths: Some(paths),
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
                paths: Some(paths),
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
#[cfg(any(windows, test))]
fn prepare_eth_package(package: &Path, backend: &dyn ThemeBackend) -> io::Result<PreparedTheme> {
    backend.require_seven_zip()?;
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
            paths: None,
            staging: None,
            components: vec![PreparedComponent::LoadScreen],
            warnings,
        });
    }

    if components.is_empty() {
        return Ok(PreparedTheme {
            source: package.to_owned(),
            kind: ThemeType::Eth,
            paths: None,
            staging: None,
            components: Vec::new(),
            warnings,
        });
    }

    let paths = backend.theme_paths()?;

    let staging = StagingDir::create(&paths)?;
    let whitelist = components
        .iter()
        .filter(|(component, _)| *component != ThemeComponent::LoadScreen)
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    backend.extract_archive_entries(package, staging.path(), &whitelist)?;
    details::archive::verify_extraction(staging.path())?;
    let staging = Some(staging);

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
                let wallpaper = details::wallpaper::prepare_wallpaper(&staged, backend)
                    .map_err(|error| component_error(*component, "precheck", error))?;
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
                )
                .map_err(|error| component_error(*component, "precheck", error))?;
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
                )
                .map_err(|error| component_error(*component, "precheck", error))?;
                prepared_components.push(PreparedComponent::MouseStyle(Box::new(mouse_style)));
            }
            ThemeComponent::StartIsBackConfig => {
                let esc = details::esc::prepare_esc(&staged)
                    .map_err(|error| component_error(*component, "precheck", error))?;
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
                )
                .map_err(|error| component_error(*component, "precheck", error))?;
                prepared_components.push(PreparedComponent::SystemIconPack(Box::new(
                    system_icon_pack,
                )));
            }
        }
    }

    Ok(PreparedTheme {
        source: package.to_owned(),
        kind: ThemeType::Eth,
        paths: Some(paths),
        staging,
        components: prepared_components,
        warnings,
    })
}

/// 提交阶段：在 named mutex 内按数据依赖提交组件，累计刷新请求，
/// 最后由统一 RefreshPlan 以最小刷新集合执行。
#[cfg(any(windows, test))]
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
    let should_log = prepared
        .components
        .iter()
        .any(|component| !matches!(component, PreparedComponent::LoadScreen));
    if !should_log {
        summary
            .components
            .extend(prepared.components.iter().map(|_| ComponentOutcome {
                component: ThemeComponent::LoadScreen,
                status: ComponentStatus::Skipped,
                windows_error_code: None,
            }));
        return Ok(summary);
    }
    let paths = prepared.paths.as_ref().ok_or_else(|| {
        io::Error::other("an applicable theme component was prepared without runtime paths")
    })?;
    let mut event_log = if should_log {
        match details::event_log::ThemeEventLog::open(paths, &prepared.source) {
            Ok(log) => Some(log),
            Err(error) => {
                summary.warnings.push(format!(
                    "failed to open the theme apply event log; continuing without it: {error}"
                ));
                None
            }
        }
    } else {
        None
    };

    // ESS 需要在统一刷新阶段（Explorer 停止期间）执行双 DLL 替换。
    let mut pending_ess: Option<usize> = None;

    for component in &prepared.components {
        let outcome = match component {
            PreparedComponent::LoadScreen => ComponentOutcome {
                component: ThemeComponent::LoadScreen,
                status: ComponentStatus::Skipped,
                windows_error_code: None,
            },
            PreparedComponent::Wallpaper(wallpaper) => {
                match details::wallpaper::commit_wallpaper(wallpaper, backend, paths) {
                    Ok(()) => ComponentOutcome {
                        component: ThemeComponent::Wallpaper,
                        status: ComponentStatus::Applied,
                        windows_error_code: None,
                    },
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::Wallpaper,
                        status: ComponentStatus::Failed(error.to_string()),
                        windows_error_code: error.raw_os_error(),
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
                            windows_error_code: None,
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::MouseStyle,
                        status: ComponentStatus::Failed(error.to_string()),
                        windows_error_code: error.raw_os_error(),
                    },
                }
            }
            PreparedComponent::IconPack(icon_pack) => {
                match details::eis::commit_eis(icon_pack, backend, paths, &mut refresh_plan) {
                    Ok((stats, link_failures)) => {
                        summary.eis = stats;
                        if stats.updated == 0 && stats.failed > 0 {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::Failed(format!(
                                    "every existing shortcut target failed to modify ({} failed): {}",
                                    stats.failed,
                                    link_failures.join("; ")
                                )),
                                windows_error_code: None,
                            }
                        } else if stats.failed > 0 {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::AppliedWithWarnings(
                                    std::iter::once(format!(
                                        "{} shortcut link(s) failed to modify; already published icons remain usable",
                                        stats.failed
                                    ))
                                    .chain(link_failures)
                                    .collect(),
                                ),
                                windows_error_code: None,
                            }
                        } else {
                            ComponentOutcome {
                                component: ThemeComponent::IconPack,
                                status: ComponentStatus::Applied,
                                windows_error_code: None,
                            }
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::IconPack,
                        status: ComponentStatus::Failed(format!(
                            "icon resources failed to publish: {error}"
                        )),
                        windows_error_code: error.raw_os_error(),
                    },
                }
            }
            PreparedComponent::StartIsBackConfig(esc) => {
                match details::esc::commit_esc(esc, backend, &mut refresh_plan) {
                    Ok(()) => ComponentOutcome {
                        component: ThemeComponent::StartIsBackConfig,
                        status: ComponentStatus::Applied,
                        windows_error_code: None,
                    },
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::StartIsBackConfig,
                        status: ComponentStatus::Failed(error.to_string()),
                        windows_error_code: error.raw_os_error(),
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
                match system_icon_pack.commit_snapshot(transaction_dir, paths) {
                    Ok(commit) => {
                        let index = summary.components.len();
                        pending_ess = Some(index);
                        refresh_plan.request(RefreshRequest::IconCacheInvalidate);
                        refresh_plan.request(RefreshRequest::SystemIconReplacement(commit));
                        ComponentOutcome {
                            component: ThemeComponent::SystemIconPack,
                            status: ComponentStatus::Applied,
                            windows_error_code: None,
                        }
                    }
                    Err(error) => ComponentOutcome {
                        component: ThemeComponent::SystemIconPack,
                        status: ComponentStatus::Failed(format!(
                            "failed to snapshot the system icon DLLs: {error}"
                        )),
                        windows_error_code: error.raw_os_error(),
                    },
                }
            }
        };
        summary.components.push(outcome);
    }

    // 统一最小刷新。
    let mut ess_error: Option<(String, Option<i32>)> = None;
    let (executed, refresh_result) =
        refresh_plan.execute_minimal(
            backend,
            paths,
            &mut |commit| match commit.replace(backend) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let message = error.to_string();
                    ess_error = Some((message.clone(), error.raw_os_error()));
                    Err(io::Error::new(error.kind(), message))
                }
            },
        );
    summary.refresh = executed;
    summary.refresh.cursors_refreshed |= cursors_refreshed;

    // 汇总 ESS 结果与整体刷新失败。
    let mut refresh_failure: Option<(String, Option<i32>)> = None;
    if let Err(error) = refresh_result {
        refresh_failure = Some((error.to_string(), error.raw_os_error()));
    }
    if let Some(index) = pending_ess {
        let failure_attr = match (ess_error, refresh_failure) {
            (Some((ess_error, ess_code)), Some((refresh_error, refresh_code))) => Some((
                format!(
                    "system icon DLL replacement failed and was rolled back: {ess_error}; the refresh phase also failed: {refresh_error}"
                ),
                ess_code.or(refresh_code),
            )),
            (Some((ess_error, ess_code)), None) => Some((
                format!("system icon DLL replacement failed and was rolled back: {ess_error}"),
                ess_code,
            )),
            (None, Some((refresh_error, refresh_code))) => Some((
                format!("system icon apply refresh phase failed: {refresh_error}"),
                refresh_code,
            )),
            (None, None) => None,
        };
        if let Some((message, error_code)) = failure_attr {
            summary.components[index].status = ComponentStatus::Failed(message);
            summary.components[index].windows_error_code = error_code;
        }
    } else if let Some((refresh_error, refresh_error_code)) = refresh_failure {
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
                    outcome.windows_error_code = refresh_error_code;
                }
                ComponentStatus::AppliedWithWarnings(_) => {
                    let mut warnings =
                        match std::mem::replace(&mut outcome.status, ComponentStatus::Applied) {
                            ComponentStatus::AppliedWithWarnings(warnings) => warnings,
                            _ => unreachable!(),
                        };
                    warnings.push(message.clone());
                    outcome.status = ComponentStatus::Failed(warnings.join("; "));
                    outcome.windows_error_code = refresh_error_code;
                }
                _ => {}
            }
            break;
        }
    }

    if let Some(log) = event_log.as_mut() {
        let log_error = summary.components.iter().find_map(|outcome| {
            let (result, detail) = match &outcome.status {
                ComponentStatus::Applied => ("applied", None),
                ComponentStatus::AppliedWithWarnings(warnings) => {
                    ("applied_with_warnings", Some(warnings.join("; ")))
                }
                ComponentStatus::Skipped => ("skipped", None),
                ComponentStatus::Failed(error) => ("failed", Some(error.clone())),
            };
            let phase = if outcome.component == ThemeComponent::SystemIconPack {
                "refresh"
            } else {
                "commit"
            };
            log.record(
                outcome.component.display_name(),
                phase,
                result,
                outcome.windows_error_code,
                detail.as_deref(),
            )
            .err()
        });
        if let Some(error) = log_error {
            summary.warnings.push(format!(
                "failed to append the theme apply event log; the apply result is unchanged: {error}"
            ));
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::test_support::{
        FakeArchive, FakeBackend, test_root as make_test_root,
    };
    use details::archive::ArchiveEntry;
    use details::refresh::{RefreshPlan, RefreshRequest};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_root() -> PathBuf {
        make_test_root("apply")
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
        let mut bytes = vec![0u8; 1024];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let (machine, magic, optional_size) = if cfg!(all(windows, target_arch = "x86")) {
            (0x14cu16, 0x10bu16, 0xe0u16)
        } else {
            (0x8664u16, 0x20bu16, 0xf0u16)
        };
        bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        bytes[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&optional_size.to_le_bytes());
        bytes[0x96..0x98].copy_from_slice(&0x2000u16.to_le_bytes());
        let optional_start = 0x98usize;
        bytes[optional_start..optional_start + 2].copy_from_slice(&magic.to_le_bytes());
        bytes[optional_start + 60..optional_start + 64].copy_from_slice(&512u32.to_le_bytes());
        let section = optional_start + optional_size as usize;
        bytes[section..section + 5].copy_from_slice(b".rsrc");
        bytes[section + 16..section + 20].copy_from_slice(&512u32.to_le_bytes());
        bytes[section + 20..section + 24].copy_from_slice(&512u32.to_le_bytes());
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
        assert!(
            !backend
                .paths
                .staging_root
                .parent()
                .unwrap()
                .join("theme-apply.log")
                .exists()
        );
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
        std::fs::create_dir_all(&backend.paths.desktop_roots[0]).unwrap();

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
        let ComponentStatus::AppliedWithWarnings(warnings) = &outcome.status else {
            unreachable!();
        };
        assert!(warnings.iter().any(|warning| warning.contains("App2.lnk")));
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
    fn refresh_summary_does_not_claim_a_failed_icon_cache_invalidation() {
        let root = test_root();
        let backend = FakeBackend::new(root.clone());
        backend.state.lock().unwrap().shell_running = true;
        std::fs::write(&backend.paths.icon_cache_dir, b"not a directory").unwrap();
        let mut plan = RefreshPlan::default();
        plan.request(RefreshRequest::ExplorerRestart);
        plan.request(RefreshRequest::IconCacheInvalidate);

        let (executed, refresh_result) =
            plan.execute_minimal(&backend, &backend.paths.clone(), &mut |_| unreachable!());

        assert!(refresh_result.is_ok());
        assert!(!executed.icon_cache_invalidated);
        assert!(!executed.warnings.is_empty());
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
