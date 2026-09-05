use crate::Ctx;
use crate::version_identifier::{EdgelessVersionIdentifier, ReleaseStage};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use isomage::{TreeNode, cat_node, iso9660::parse_iso9660, udf::parse_udf};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const WIM_MAGIC: [u8; 8] = [b'M', b'S', b'W', b'I', b'M', 0, 0, 0];

/// 内核保存命令的执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreResult {
    /// 保存后的 WIM 文件路径。
    pub wim_path: PathBuf,
    /// ISO 输入同时更新了 Edgeless 依赖目录。
    pub updated_edgeless: bool,
    /// 已识别的 Edgeless 内核版本。
    pub version: EdgelessVersionIdentifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Iso,
    Wim,
}

#[derive(Debug)]
struct StorePlan {
    kind: InputKind,
    version: EdgelessVersionIdentifier,
}

/// 将 Edgeless ISO 或 WIM 保存到选中的启动盘。
///
/// ISO 会更新其中的 `Edgeless` 目录和 `sources/boot.wim`；WIM 则只更新根目录的
/// 同名启动文件。所有有效载荷先写入启动盘内的私有暂存目录，再在 ELI 的跨进程锁内发布，
/// 以避免其他 ELI 写入命令看到不完整的 WIM。`name` 用于本地文件被改名时指定原始发布名。
pub fn store(ctx: &Ctx, source: &Path, name: Option<&str>) -> io::Result<StoreResult> {
    let plan = build_plan(source, name)?;
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    let _write_lock = crate::command::bootdisk::acquire_write_lock(&bootdisk.mount_point)?;
    recover_interrupted_publish(&bootdisk.mount_point)?;

    match plan.kind {
        InputKind::Iso => store_iso(&bootdisk.mount_point, source, plan),
        InputKind::Wim => store_wim(&bootdisk.mount_point, source, plan),
    }
}

fn build_plan(source: &Path, name: Option<&str>) -> io::Result<StorePlan> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to inspect kernel source {}: {error}",
                source.display()
            ),
        )
    })?;
    if !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kernel source is not a regular file: {}", source.display()),
        ));
    }

    let kind = input_kind(source)?;
    let logical_name = match name {
        Some(name) => name.to_owned(),
        None => source
            .file_name()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("kernel source has no UTF-8 file name: {}", source.display()),
                )
            })?,
    };
    let version = validate_logical_name(&logical_name, kind)?;

    Ok(StorePlan { kind, version })
}

fn input_kind(source: &Path) -> io::Result<InputKind> {
    let extension = source.extension().and_then(OsStr::to_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kernel source has no UTF-8 extension: {}", source.display()),
        )
    })?;
    if extension.eq_ignore_ascii_case("iso") {
        Ok(InputKind::Iso)
    } else if extension.eq_ignore_ascii_case("wim") {
        Ok(InputKind::Wim)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "kernel source must use the .iso or .wim extension: {}",
                source.display()
            ),
        ))
    }
}

fn validate_logical_name(name: &str, kind: InputKind) -> io::Result<EdgelessVersionIdentifier> {
    let expected_extension = match kind {
        InputKind::Iso => "iso",
        InputKind::Wim => "wim",
    };
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid Edgeless release file name: {name:?}"),
        ));
    }
    let extension = Path::new(name).extension().and_then(OsStr::to_str);
    if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case(expected_extension)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Edgeless release file name must use the .{expected_extension} extension: {name:?}"
            ),
        ));
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid Edgeless release file name: {name:?}"),
            )
        })?;
    EdgelessVersionIdentifier::parse(stem).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid Edgeless release file name {name:?}: {error}"),
        )
    })
}

fn store_wim(root: &Path, source: &Path, plan: StorePlan) -> io::Result<StoreResult> {
    validate_wim_header(source)?;
    let total = fs::metadata(source)?.len();
    let progress = StoreProgress::visible(total, "正在暂存内核 WIM");
    let staging = StagingDirectory::new(root)?;
    let staged_wim = staging.path.join("boot.wim");
    let result = (|| {
        copy_file_with_progress(source, &staged_wim, &progress)?;
        validate_wim_header(&staged_wim)?;
        let destination = root.join(wim_name(source)?);
        publish(root, None, &staged_wim, &destination, &progress)?;
        progress.finish("内核保存完成");
        Ok(StoreResult {
            wim_path: destination,
            updated_edgeless: false,
            version: plan.version,
        })
    })();
    staging.cleanup();
    if result.is_err() {
        progress.fail("内核保存失败");
    }
    result
}

fn store_iso(root: &Path, source: &Path, plan: StorePlan) -> io::Result<StoreResult> {
    let mut iso = File::open(source)?;
    let tree = parse_edgeless_iso(&mut iso, source)?;
    let (edgeless, boot_wim) = required_iso_content(&tree)?;
    let total = directory_payload_size(edgeless)?
        .checked_add(file_payload_size(boot_wim)?)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ISO payload size overflow"))?;
    let progress = StoreProgress::visible(total, "正在保存 Edgeless 文件");
    let staging = StagingDirectory::new(root)?;
    let staged_edgeless = staging.path.join("Edgeless");
    let staged_wim = staging.path.join("boot.wim");
    let result = (|| {
        extract_directory(&mut iso, edgeless, &staged_edgeless, &progress)?;
        progress.message("正在保存内核 WIM");
        extract_file(&mut iso, boot_wim, &staged_wim, &progress)?;
        validate_wim_header(&staged_wim)?;
        let stored_version = read_staged_version(&staged_edgeless.join("version.txt"))?;
        if stored_version != plan.version {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "ISO Edgeless/version.txt identifies {stored_version}, but release name identifies {}",
                    plan.version
                ),
            ));
        }

        let destination = root.join(wim_name(source)?);
        publish(
            root,
            Some(&staged_edgeless),
            &staged_wim,
            &destination,
            &progress,
        )?;
        progress.finish("内核保存完成");
        Ok(StoreResult {
            wim_path: destination,
            updated_edgeless: true,
            version: plan.version,
        })
    })();
    staging.cleanup();
    if result.is_err() {
        progress.fail("内核保存失败");
    }
    result
}

/// 同时包含 ISO9660 与 UDF 目录树的镜像，必须选择真正含 Edgeless 有效载荷的目录树。
fn parse_edgeless_iso(iso: &mut File, source: &Path) -> io::Result<TreeNode> {
    let mut errors = Vec::new();
    for (format_name, result) in [("ISO9660", parse_iso9660(iso)), ("UDF", parse_udf(iso))] {
        match result {
            Ok(tree) => match required_iso_content(&tree) {
                Ok(_) => return Ok(tree),
                Err(error) => errors.push(format!("{format_name}: {error}")),
            },
            Err(error) => errors.push(format!("{format_name}: {error}")),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "failed to find Edgeless content in ISO {}: {}",
            source.display(),
            errors.join("; ")
        ),
    ))
}

fn required_iso_content(tree: &TreeNode) -> io::Result<(&TreeNode, &TreeNode)> {
    let edgeless = tree.find_node("Edgeless").ok_or_else(|| {
        let entries = tree
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ISO does not contain Edgeless directory; parsed root entries: {entries}"),
        )
    })?;
    if !edgeless.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ISO Edgeless entry is not a directory",
        ));
    }
    let version = edgeless.find_node("version.txt").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "ISO Edgeless directory does not contain version.txt",
        )
    })?;
    if version.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ISO Edgeless/version.txt is not a file",
        ));
    }
    let boot_wim = tree.find_node("sources/boot.wim").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "ISO does not contain sources/boot.wim",
        )
    })?;
    if boot_wim.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ISO sources/boot.wim is not a file",
        ));
    }
    validate_tree(edgeless)?;
    validate_file_node(boot_wim)?;
    Ok((edgeless, boot_wim))
}

fn validate_tree(node: &TreeNode) -> io::Result<()> {
    for child in &node.children {
        validate_node_name(&child.name)?;
        if child.is_directory {
            validate_tree(child)?;
        } else {
            validate_file_node(child)?;
        }
    }
    Ok(())
}

fn validate_file_node(node: &TreeNode) -> io::Result<()> {
    if node.is_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ISO entry is a directory, not a file: {}", node.name),
        ));
    }
    validate_node_name(&node.name)?;
    let _ = file_payload_size(node)?;
    Ok(())
}

fn validate_node_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ISO contains an unsafe entry name: {name:?}"),
        ));
    }
    Ok(())
}

fn directory_payload_size(node: &TreeNode) -> io::Result<u64> {
    let mut total = 0_u64;
    for child in &node.children {
        let size = if child.is_directory {
            directory_payload_size(child)?
        } else {
            file_payload_size(child)?
        };
        total = total.checked_add(size).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "ISO directory size overflow")
        })?;
    }
    Ok(total)
}

fn file_payload_size(node: &TreeNode) -> io::Result<u64> {
    node.file_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ISO file has no byte range: {}", node.name),
        )
    })
}

fn extract_directory(
    iso: &mut File,
    directory: &TreeNode,
    destination: &Path,
    progress: &StoreProgress,
) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for child in &directory.children {
        validate_node_name(&child.name)?;
        let child_destination = destination.join(&child.name);
        if child.is_directory {
            extract_directory(iso, child, &child_destination, progress)?;
        } else {
            extract_file(iso, child, &child_destination, progress)?;
        }
    }
    Ok(())
}

fn extract_file(
    iso: &mut File,
    node: &TreeNode,
    destination: &Path,
    progress: &StoreProgress,
) -> io::Result<()> {
    validate_file_node(node)?;
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("staged ISO file has no parent: {}", destination.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut writer = ProgressWriter { output, progress };
    cat_node(iso, node, &mut writer).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to extract ISO file {}: {error}", node.name),
        )
    })?;
    writer.output.sync_all()?;
    let actual = writer.output.metadata()?.len();
    let expected = file_payload_size(node)?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "incomplete ISO extraction for {}: expected {expected} bytes, got {actual}",
                node.name
            ),
        ));
    }
    Ok(())
}

fn read_staged_version(path: &Path) -> io::Result<EdgelessVersionIdentifier> {
    let value = fs::read_to_string(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read ISO version file {}: {error}",
                path.display()
            ),
        )
    })?;
    value.trim().parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "invalid Edgeless version in ISO {}: {error}",
                path.display()
            ),
        )
    })
}

fn validate_wim_header(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; WIM_MAGIC.len()];
    file.read_exact(&mut header).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to read WIM header {}: {error}", path.display()),
        )
    })?;
    if header != WIM_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("kernel source is not a WIM file: {}", path.display()),
        ));
    }
    Ok(())
}

/// WIM 输出名称始终跟随实际输入文件名；`--name` 只参与版本解析。
fn wim_name(source: &Path) -> io::Result<String> {
    let stem = source.file_stem().and_then(OsStr::to_str).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kernel source has no UTF-8 file stem: {}", source.display()),
        )
    })?;
    if stem.is_empty() || stem.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "kernel source has an unsafe file stem: {}",
                source.display()
            ),
        ));
    }
    Ok(format!("{stem}.wim"))
}

fn copy_file_with_progress(
    source: &Path,
    destination: &Path,
    progress: &StoreProgress,
) -> io::Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("staged WIM has no parent: {}", destination.display()),
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        progress.advance(count as u64);
    }
    output.sync_all()
}

fn publish(
    root: &Path,
    staged_edgeless: Option<&Path>,
    staged_wim: &Path,
    destination_wim: &Path,
    progress: &StoreProgress,
) -> io::Result<()> {
    let backup_root = staged_wim
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "staged WIM has no parent"))?
        .join("backup");
    let mut transaction = PublishTransaction::new(root, backup_root)?;
    let result = (|| {
        if let Some(staged_edgeless) = staged_edgeless {
            disable_previous_beta_wims(root, destination_wim, &mut transaction)?;
            publish_directory(
                staged_edgeless,
                &root.join("Edgeless"),
                Path::new("Edgeless"),
                &mut transaction,
                progress,
            )?;
        }
        let wim_size = fs::metadata(staged_wim)?.len();
        transaction.publish_file(staged_wim, destination_wim, Path::new("kernel.wim"))?;
        progress.advance_publish(wim_size);
        transaction.commit()?;
        Ok(())
    })();
    if let Err(error) = result {
        return match transaction.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(io::Error::other(format!(
                "{error}; failed to restore the previous kernel files: {rollback_error}"
            ))),
        };
    }
    Ok(())
}

/// 禁用当前 ISO 之外的旧 Beta 内核，使启动盘只保留一个已启用的 Beta WIM。
fn disable_previous_beta_wims(
    root: &Path,
    destination_wim: &Path,
    transaction: &mut PublishTransaction,
) -> io::Result<()> {
    let mut entries = fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path == destination_wim {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || is_reparse_point(&metadata) || !is_beta_wim(&path) {
            continue;
        }
        transaction.disable_file(&path)?;
    }
    Ok(())
}

fn is_beta_wim(path: &Path) -> bool {
    let is_wim = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wim"));
    is_wim
        && path
            .file_stem()
            .and_then(OsStr::to_str)
            .and_then(|stem| EdgelessVersionIdentifier::parse(stem).ok())
            .is_some_and(|identifier| identifier.stage == ReleaseStage::Beta)
}

fn publish_directory(
    staged: &Path,
    destination: &Path,
    relative: &Path,
    transaction: &mut PublishTransaction,
    progress: &StoreProgress,
) -> io::Result<()> {
    ensure_real_directory(destination)?;
    for entry in fs::read_dir(staged)? {
        let entry = entry?;
        let source = entry.path();
        let name = entry.file_name();
        let target = destination.join(&name);
        let relative_target = relative.join(&name);
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() && !is_reparse_point(&metadata) {
            publish_directory(&source, &target, &relative_target, transaction, progress)?;
        } else if metadata.is_file() && !is_reparse_point(&metadata) {
            let size = metadata.len();
            transaction.publish_file(&source, &target, &relative_target)?;
            progress.advance_publish(size);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("staging contains an unsafe entry: {}", source.display()),
            ));
        }
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !is_reparse_point(&metadata) => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "kernel destination is not a real directory: {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(path),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
enum PublishChange {
    Created {
        destination: PathBuf,
    },
    Replaced {
        destination: PathBuf,
        backup: PathBuf,
    },
    Disabled {
        source: PathBuf,
        destination: PathBuf,
    },
}

#[derive(Debug)]
struct PublishTransaction {
    root: PathBuf,
    backup_root: PathBuf,
    journal: File,
    changes: Vec<PublishChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JournalRecord {
    Change(JournalChange),
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum JournalChange {
    Created {
        destination: PathBuf,
    },
    Replaced {
        destination: PathBuf,
        backup: PathBuf,
    },
    Disabled {
        source: PathBuf,
        destination: PathBuf,
    },
}

impl PublishTransaction {
    fn new(root: &Path, backup_root: PathBuf) -> io::Result<Self> {
        let journal_path = backup_root
            .parent()
            .expect("backup root always has a staging parent")
            .join("transaction.jsonl");
        let journal = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(journal_path)?;
        Ok(Self {
            root: root.to_owned(),
            backup_root,
            journal,
            changes: Vec::new(),
        })
    }

    fn publish_file(
        &mut self,
        source: &Path,
        destination: &Path,
        backup_name: &Path,
    ) -> io::Result<()> {
        if let Some(metadata) = optional_metadata(destination)? {
            if !metadata.is_file() || is_reparse_point(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "kernel destination is not a regular file: {}",
                        destination.display()
                    ),
                ));
            }
            let backup = self.backup_root.join(backup_name);
            self.record(JournalChange::Replaced {
                destination: self.relative_to_root(destination)?,
                backup: self.relative_to_staging(&backup)?,
            })?;
            let parent = backup.parent().expect("backup path always has a parent");
            fs::create_dir_all(parent)?;
            fs::rename(destination, &backup)?;
            if let Err(error) = fs::rename(source, destination) {
                let restore = fs::rename(&backup, destination);
                if let Err(restore_error) = restore {
                    return Err(io::Error::other(format!(
                        "failed to publish {}: {error}; failed to restore {}: {restore_error}",
                        destination.display(),
                        destination.display()
                    )));
                }
                return Err(error);
            }
            self.changes.push(PublishChange::Replaced {
                destination: destination.to_owned(),
                backup,
            });
        } else {
            self.record(JournalChange::Created {
                destination: self.relative_to_root(destination)?,
            })?;
            fs::rename(source, destination)?;
            self.changes.push(PublishChange::Created {
                destination: destination.to_owned(),
            });
        }
        Ok(())
    }

    fn disable_file(&mut self, source: &Path) -> io::Result<()> {
        let mut disabled_name = source.as_os_str().to_owned();
        disabled_name.push("bak");
        let destination = PathBuf::from(disabled_name);
        if optional_metadata(&destination)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "disabled Beta kernel already exists: {}",
                    destination.display()
                ),
            ));
        }
        self.record(JournalChange::Disabled {
            source: self.relative_to_root(source)?,
            destination: self.relative_to_root(&destination)?,
        })?;
        fs::rename(source, &destination)?;
        self.changes.push(PublishChange::Disabled {
            source: source.to_owned(),
            destination,
        });
        Ok(())
    }

    fn commit(&mut self) -> io::Result<()> {
        self.write_journal(&JournalRecord::Committed)?;
        self.changes.clear();
        Ok(())
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut failures = Vec::new();
        for change in self.changes.drain(..).rev() {
            let result = match change {
                PublishChange::Created { destination } => remove_file_if_exists(&destination),
                PublishChange::Replaced {
                    destination,
                    backup,
                } => {
                    let mut errors = Vec::new();
                    if let Err(error) = remove_file_if_exists(&destination) {
                        errors.push(format!("remove {}: {error}", destination.display()));
                    }
                    if let Err(error) = fs::rename(&backup, &destination) {
                        errors.push(format!(
                            "restore {} from {}: {error}",
                            destination.display(),
                            backup.display()
                        ));
                    }
                    if errors.is_empty() {
                        Ok(())
                    } else {
                        Err(io::Error::other(errors.join("; ")))
                    }
                }
                PublishChange::Disabled {
                    source,
                    destination,
                } => fs::rename(destination, source),
            };
            if let Err(error) = result {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
    }

    fn record(&mut self, change: JournalChange) -> io::Result<()> {
        self.write_journal(&JournalRecord::Change(change))
    }

    fn write_journal(&mut self, record: &JournalRecord) -> io::Result<()> {
        serde_json::to_writer(&mut self.journal, record).map_err(io::Error::other)?;
        self.journal.write_all(b"\n")?;
        self.journal.sync_data()
    }

    fn relative_to_root(&self, path: &Path) -> io::Result<PathBuf> {
        path.strip_prefix(&self.root)
            .map(Path::to_owned)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("publish path is outside boot disk: {}", path.display()),
                )
            })
    }

    fn relative_to_staging(&self, path: &Path) -> io::Result<PathBuf> {
        path.strip_prefix(
            self.backup_root
                .parent()
                .expect("backup root always has a staging parent"),
        )
        .map(Path::to_owned)
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "backup path is outside staging directory: {}",
                    path.display()
                ),
            )
        })
    }
}

fn optional_metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 恢复先前进程在发布阶段被中断时遗留的未提交事务。
///
/// 暂存目录和发布目标位于同一个启动盘上。每次替换前都会同步写入撤销记录，
/// 所以只要事务未标记完成，就能以旧文件恢复目标；已经提交的事务则保留新文件。
fn recover_interrupted_publish(root: &Path) -> io::Result<()> {
    let entries = fs::read_dir(root)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".eli-kernel-store-") {
            continue;
        }
        let staging = entry.path();
        let metadata = fs::symlink_metadata(&staging)?;
        if !metadata.is_dir() || is_reparse_point(&metadata) {
            continue;
        }
        let journal_path = staging.join("transaction.jsonl");
        if !journal_path.is_file() {
            continue;
        }
        let (changes, committed) = read_transaction_journal(&journal_path)?;
        if !committed {
            restore_interrupted_publish(root, &staging, &changes)?;
        }
        fs::remove_dir_all(&staging)?;
    }
    Ok(())
}

fn read_transaction_journal(path: &Path) -> io::Result<(Vec<JournalChange>, bool)> {
    let file = File::open(path)?;
    let mut changes = Vec::new();
    let mut committed = false;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<JournalRecord>(&line).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid interrupted kernel-store transaction {}: {error}",
                    path.display()
                ),
            )
        })? {
            JournalRecord::Change(change) => changes.push(change),
            JournalRecord::Committed => committed = true,
        }
    }
    Ok((changes, committed))
}

fn restore_interrupted_publish(
    root: &Path,
    staging: &Path,
    changes: &[JournalChange],
) -> io::Result<()> {
    let mut failures = Vec::new();
    for change in changes.iter().rev() {
        let result = match change {
            JournalChange::Created { destination } => {
                let destination = safe_journal_path(root, destination)?;
                remove_file_if_exists(&destination)
            }
            JournalChange::Replaced {
                destination,
                backup,
            } => {
                let destination = safe_journal_path(root, destination)?;
                let backup = safe_journal_path(staging, backup)?;
                if !backup.is_file() {
                    Ok(())
                } else {
                    let mut errors = Vec::new();
                    if let Err(error) = remove_file_if_exists(&destination) {
                        errors.push(format!("remove {}: {error}", destination.display()));
                    }
                    if let Err(error) = fs::rename(&backup, &destination) {
                        errors.push(format!(
                            "restore {} from {}: {error}",
                            destination.display(),
                            backup.display()
                        ));
                    }
                    if errors.is_empty() {
                        Ok(())
                    } else {
                        Err(io::Error::other(errors.join("; ")))
                    }
                }
            }
            JournalChange::Disabled {
                source,
                destination,
            } => {
                let source = safe_journal_path(root, source)?;
                let destination = safe_journal_path(root, destination)?;
                if destination.exists() {
                    fs::rename(destination, source)
                } else {
                    Ok(())
                }
            }
        };
        if let Err(error) = result {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

fn safe_journal_path(base: &Path, relative: &Path) -> io::Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsafe kernel-store transaction path: {}",
                relative.display()
            ),
        ));
    }
    Ok(base.join(relative))
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

struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    fn new(root: &Path) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".eli-kernel-store-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StoreProgress {
    bar: ProgressBar,
    publish_weight: u64,
}

impl StoreProgress {
    fn visible(staging_bytes: u64, message: &'static str) -> Self {
        let publish_weight = staging_bytes.saturating_div(5).max(1);
        let bar = ProgressBar::new(staging_bytes.saturating_add(publish_weight));
        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(10));
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {percent}% ({per_sec}, {eta}) {msg}",
            )
            .expect("进度条模板固定有效")
            .progress_chars("#>-"),
        );
        bar.set_message(message);
        bar.enable_steady_tick(std::time::Duration::from_millis(100));
        Self {
            bar,
            publish_weight,
        }
    }

    fn advance(&self, bytes: u64) {
        self.bar.inc(bytes);
    }

    /// 发布阶段主要是同盘重命名和元数据更新，按有效载荷的估算权重计入进度。
    fn advance_publish(&self, bytes: u64) {
        self.bar
            .inc(bytes.saturating_div(5).min(self.publish_weight));
    }

    fn finish(&self, message: &'static str) {
        self.bar.finish_with_message(message);
    }

    fn message(&self, message: &'static str) {
        self.bar.set_message(message);
    }

    fn fail(&self, message: &'static str) {
        self.bar.abandon_with_message(message);
    }
}

struct ProgressWriter<'a> {
    output: File,
    progress: &'a StoreProgress,
}

impl Write for ProgressWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = self.output.write(buffer)?;
        self.progress.advance(count as u64);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_a_logical_iso_name() {
        let version =
            validate_logical_name("Edgeless_Beta_Ofial_4.1.0_2.iso", InputKind::Iso).unwrap();

        assert_eq!(version.to_string(), "Edgeless_Beta_Ofial_4.1.0_2");
    }

    #[test]
    fn rejects_a_logical_name_with_a_path() {
        let error =
            validate_logical_name("nested/Edgeless_Beta_4.1.0.iso", InputKind::Iso).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn rejects_a_logical_name_with_the_wrong_extension() {
        let error = validate_logical_name("Edgeless_Beta_4.1.0.wim", InputKind::Iso).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn uses_the_actual_source_stem_for_the_wim_name() {
        assert_eq!(
            wim_name(Path::new("Edgeless_Beta_4.1.0.iso")).unwrap(),
            "Edgeless_Beta_4.1.0.wim"
        );
    }

    #[test]
    fn stores_a_wim_without_removing_the_source() {
        let root = test_root();
        fs::create_dir_all(root.join("Edgeless")).unwrap();
        let source = root.join("source.wim");
        fs::write(
            &source,
            WIM_MAGIC
                .into_iter()
                .chain(b"payload".iter().copied())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let plan = StorePlan {
            kind: InputKind::Wim,
            version: EdgelessVersionIdentifier::parse("Edgeless_Beta_Ofial_4.1.0_2").unwrap(),
        };

        let result = store_wim(&root, &source, plan).unwrap();

        assert_eq!(fs::read(&source).unwrap()[..8], WIM_MAGIC);
        assert_eq!(fs::read(&result.wim_path).unwrap()[..8], WIM_MAGIC);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stores_an_iso_and_preserves_existing_resource_files() {
        let root = test_root();
        let resource = root.join("Edgeless").join("Resource");
        fs::create_dir_all(&resource).unwrap();
        fs::write(resource.join("existing.7z"), b"plugin").unwrap();
        let previous_wim = root.join("Edgeless_Beta_3.9.0.wim");
        fs::write(&previous_wim, WIM_MAGIC).unwrap();
        let source = root.join("renamed.iso");
        write_minimal_edgeless_iso(&source);
        let plan = StorePlan {
            kind: InputKind::Iso,
            version: EdgelessVersionIdentifier::parse("Edgeless_Beta_Ofial_4.1.0_2").unwrap(),
        };

        let result = store_iso(&root, &source, plan).unwrap();

        assert!(result.updated_edgeless);
        assert_eq!(
            fs::read_to_string(root.join("Edgeless").join("version.txt")).unwrap(),
            "Edgeless_Beta_Ofial_4.1.0_2"
        );
        assert_eq!(fs::read(resource.join("existing.7z")).unwrap(), b"plugin");
        assert_eq!(fs::read(&result.wim_path).unwrap()[..8], WIM_MAGIC);
        assert!(!previous_wim.exists());
        assert_eq!(
            fs::read(root.join("Edgeless_Beta_3.9.0.wimbak")).unwrap(),
            WIM_MAGIC
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_wim_without_its_magic_header() {
        let root = test_root();
        let path = root.join("invalid.wim");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"not a WIM").unwrap();

        let error = validate_wim_header(&path).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_an_iso_with_a_non_wim_boot_file() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let source = root.join("renamed.iso");
        write_minimal_edgeless_iso_with_wim(&source, b"not a WIM");
        let plan = StorePlan {
            kind: InputKind::Iso,
            version: EdgelessVersionIdentifier::parse("Edgeless_Beta_Ofial_4.1.0_2").unwrap(),
        };

        let error = store_iso(&root, &source, plan).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!root.join("Edgeless").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn directory_size_sums_only_file_payloads() {
        let tree = TreeNode {
            name: "Edgeless".to_owned(),
            size: 0,
            is_directory: true,
            children: vec![TreeNode {
                name: "version.txt".to_owned(),
                size: 3,
                is_directory: false,
                children: Vec::new(),
                file_location: Some(0),
                file_length: Some(3),
            }],
            file_location: None,
            file_length: None,
        };

        assert_eq!(directory_payload_size(&tree).unwrap(), 3);
    }

    #[test]
    fn publish_transaction_restores_a_replaced_file() {
        let root = test_root();
        fs::create_dir_all(root.join("backup")).unwrap();
        let source = root.join("staged.wim");
        let destination = root.join("Edgeless_Beta_4.1.0.wim");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let mut transaction = PublishTransaction::new(&root, root.join("backup")).unwrap();

        transaction
            .publish_file(&source, &destination, Path::new("kernel.wim"))
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        transaction.rollback().unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_an_interrupted_publish_before_the_next_store() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("Edgeless_Beta_4.1.0.wim");
        fs::write(&destination, b"old").unwrap();
        let staging = StagingDirectory::new(&root).unwrap();
        let source = staging.path.join("boot.wim");
        fs::write(&source, b"new").unwrap();
        {
            let mut transaction =
                PublishTransaction::new(&root, staging.path.join("backup")).unwrap();
            transaction
                .publish_file(&source, &destination, Path::new("kernel.wim"))
                .unwrap();
            assert_eq!(fs::read(&destination).unwrap(), b"new");
        }

        recover_interrupted_publish(&root).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert!(!staging.path.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollback_continues_after_one_change_cannot_be_removed() {
        let root = test_root();
        fs::create_dir_all(root.join("backup")).unwrap();
        let destination = root.join("Edgeless_Beta_4.1.0.wim");
        let backup = root.join("backup").join("kernel.wim");
        let blocked = root.join("blocked");
        fs::write(&destination, b"new").unwrap();
        fs::write(&backup, b"old").unwrap();
        fs::create_dir(&blocked).unwrap();
        let mut transaction = PublishTransaction::new(&root, root.join("backup")).unwrap();
        transaction.changes = vec![
            PublishChange::Replaced {
                destination: destination.clone(),
                backup,
            },
            PublishChange::Created {
                destination: blocked,
            },
        ];

        assert!(transaction.rollback().is_err());

        assert_eq!(fs::read(&destination).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-kernel-store-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn write_minimal_edgeless_iso(path: &Path) {
        let wim = WIM_MAGIC
            .into_iter()
            .chain(b"kernel".iter().copied())
            .collect::<Vec<_>>();
        write_minimal_edgeless_iso_with_wim(path, &wim);
    }

    fn write_minimal_edgeless_iso_with_wim(path: &Path, wim: &[u8]) {
        const SECTOR_SIZE: usize = 2048;
        let version = b"Edgeless_Beta_Ofial_4.1.0_2";
        let mut image = vec![0_u8; SECTOR_SIZE * 23];
        let root_record = directory_record(18, SECTOR_SIZE as u32, true, &[0]);
        {
            let descriptor = sector_mut(&mut image, 16);
            descriptor[0] = 1;
            descriptor[1..6].copy_from_slice(b"CD001");
            descriptor[6] = 1;
            write_both_u32(descriptor, 80, 23);
            write_both_u16(descriptor, 128, SECTOR_SIZE as u16);
            descriptor[156..156 + root_record.len()].copy_from_slice(&root_record);
        }
        {
            let terminator = sector_mut(&mut image, 17);
            terminator[0] = 255;
            terminator[1..6].copy_from_slice(b"CD001");
            terminator[6] = 1;
        }
        write_directory(
            sector_mut(&mut image, 18),
            &[
                directory_record(18, SECTOR_SIZE as u32, true, &[0]),
                directory_record(18, SECTOR_SIZE as u32, true, &[1]),
                directory_record(19, SECTOR_SIZE as u32, true, b"Edgeless"),
                directory_record(20, SECTOR_SIZE as u32, true, b"sources"),
            ],
        );
        write_directory(
            sector_mut(&mut image, 19),
            &[
                directory_record(19, SECTOR_SIZE as u32, true, &[0]),
                directory_record(18, SECTOR_SIZE as u32, true, &[1]),
                directory_record(21, version.len() as u32, false, b"version.txt;1"),
            ],
        );
        write_directory(
            sector_mut(&mut image, 20),
            &[
                directory_record(20, SECTOR_SIZE as u32, true, &[0]),
                directory_record(18, SECTOR_SIZE as u32, true, &[1]),
                directory_record(22, wim.len() as u32, false, b"boot.wim;1"),
            ],
        );
        sector_mut(&mut image, 21)[..version.len()].copy_from_slice(version);
        sector_mut(&mut image, 22)[..wim.len()].copy_from_slice(wim);
        fs::write(path, image).unwrap();
    }

    fn sector_mut(image: &mut [u8], sector: usize) -> &mut [u8] {
        const SECTOR_SIZE: usize = 2048;
        &mut image[sector * SECTOR_SIZE..(sector + 1) * SECTOR_SIZE]
    }

    fn write_directory(directory: &mut [u8], records: &[Vec<u8>]) {
        let mut offset = 0;
        for record in records {
            directory[offset..offset + record.len()].copy_from_slice(record);
            offset += record.len();
        }
    }

    fn directory_record(extent: u32, length: u32, is_directory: bool, name: &[u8]) -> Vec<u8> {
        let padding = usize::from(name.len().is_multiple_of(2));
        let mut record = vec![0_u8; 33 + name.len() + padding];
        record[0] = record.len() as u8;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[6..10].copy_from_slice(&extent.to_be_bytes());
        record[10..14].copy_from_slice(&length.to_le_bytes());
        record[14..18].copy_from_slice(&length.to_be_bytes());
        record[18..25].copy_from_slice(&[124, 1, 1, 0, 0, 0, 0]);
        record[25] = u8::from(is_directory) << 1;
        write_both_u16(&mut record, 28, 1);
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    fn write_both_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        buffer[offset + 2..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn write_both_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        buffer[offset + 4..offset + 8].copy_from_slice(&value.to_be_bytes());
    }
}
