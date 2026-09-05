pub(crate) mod bootdisk;
pub(crate) mod config;
pub(crate) mod plugin;

use eli_lib::command::bootdisk::{BootDiskSelection, BootDiskSelectionSource};

pub(crate) fn warn_automatic_bootdisk_selection(selection: &BootDiskSelection) {
    if selection.source == BootDiskSelectionSource::Automatic && selection.candidates.len() > 1 {
        eprintln!(
            "warning: found {} Edgeless boot disks; automatically selected {}; use --bootdisk <PARTITION> to override",
            selection.candidates.len(),
            selection.selected.partition.display()
        );
    }
}
