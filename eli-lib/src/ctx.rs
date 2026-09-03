use crate::command::bootdisk::{self, BootDisk, BootDiskSelection, BootDiskSelectionSource};
use crate::dependency::DependencyManager;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 单次 `eli` 调用共享的上下文。
///
/// 全局配置集中存放于此，使各命令能够解析并复用相同资源，而不是各自独立选择。
#[derive(Debug)]
pub struct Ctx {
    bootdisk_override: Option<PathBuf>,
    bootdisk: OnceLock<BootDiskSelection>,
    dependencies: DependencyManager,
}

impl Ctx {
    pub fn new(bootdisk_override: Option<PathBuf>) -> Self {
        Self {
            bootdisk_override,
            bootdisk: OnceLock::new(),
            dependencies: DependencyManager::new(),
        }
    }

    pub fn bootdisk_override(&self) -> Option<&Path> {
        self.bootdisk_override.as_deref()
    }

    pub fn dependencies(&self) -> &DependencyManager {
        &self.dependencies
    }

    /// 返回本次调用选中的启动盘，且只解析一次。
    pub fn bootdisk(&self) -> io::Result<&BootDiskSelection> {
        if let Some(selection) = self.bootdisk.get() {
            return Ok(selection);
        }

        let selection = bootdisk::get(self.bootdisk_override())?;
        // 如果其他线程在发现过程中完成了上下文初始化，则保留已经缓存的选择结果。
        let _ = self.bootdisk.set(selection);
        Ok(self
            .bootdisk
            .get()
            .expect("boot disk selection was initialized"))
    }

    /// 为会修改或删除启动盘内容的操作选择启动盘。
    ///
    /// 连接多个 Edgeless 启动盘时，破坏性操作不得依赖自动决胜规则。
    pub fn bootdisk_for_destructive_operation(&self) -> io::Result<&BootDisk> {
        let selection = self.bootdisk()?;
        require_unambiguous_bootdisk(selection)
    }
}

fn require_unambiguous_bootdisk(selection: &BootDiskSelection) -> io::Result<&BootDisk> {
    if selection.source == BootDiskSelectionSource::Automatic && selection.candidates.len() > 1 {
        let candidates = selection
            .candidates
            .iter()
            .map(|disk| disk.partition.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "found multiple Edgeless boot disks ({candidates}); use --bootdisk <PARTITION> to select one"
            ),
        ));
    }

    Ok(&selection.selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(partition: &str) -> BootDisk {
        BootDisk {
            partition: PathBuf::from(partition),
            mount_point: PathBuf::from(partition),
            version: "test".to_owned(),
        }
    }

    #[test]
    fn destructive_operation_accepts_an_explicit_selection() {
        let selected = disk("/media/a");
        let selection = BootDiskSelection {
            selected: selected.clone(),
            candidates: vec![selected.clone()],
            source: BootDiskSelectionSource::Explicit,
        };

        assert_eq!(require_unambiguous_bootdisk(&selection).unwrap(), &selected);
    }

    #[test]
    fn destructive_operation_accepts_one_automatic_candidate() {
        let selected = disk("/media/a");
        let selection = BootDiskSelection {
            selected: selected.clone(),
            candidates: vec![selected.clone()],
            source: BootDiskSelectionSource::Automatic,
        };

        assert_eq!(require_unambiguous_bootdisk(&selection).unwrap(), &selected);
    }

    #[test]
    fn destructive_operation_rejects_multiple_automatic_candidates() {
        let selected = disk("/media/z");
        let selection = BootDiskSelection {
            selected,
            candidates: vec![disk("/media/a"), disk("/media/z")],
            source: BootDiskSelectionSource::Automatic,
        };

        let error = require_unambiguous_bootdisk(&selection).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("--bootdisk"));
        assert!(error.to_string().contains("/media/a"));
        assert!(error.to_string().contains("/media/z"));
    }

    #[test]
    fn reuses_the_cached_bootdisk_selection() {
        let selected = disk("/media/a");
        let selection = BootDiskSelection {
            selected: selected.clone(),
            candidates: vec![selected],
            source: BootDiskSelectionSource::Automatic,
        };
        let ctx = Ctx::new(None);
        ctx.bootdisk.set(selection).unwrap();

        let first = ctx.bootdisk().unwrap();
        let second = ctx.bootdisk().unwrap();

        assert!(std::ptr::eq(first, second));
    }
}
