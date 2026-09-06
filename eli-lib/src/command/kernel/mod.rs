pub mod alpha;
mod download;
mod store;
mod version;

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

pub use download::{DownloadResult, download};
pub use store::{StoreResult, store};
pub use version::{bootdisk, current, latest};

pub(crate) const WIM_MAGIC: [u8; 8] = [b'M', b'S', b'W', b'I', b'M', 0, 0, 0];

/// 校验文件是否具有 WIM 文件头。
pub(crate) fn validate_wim_header(path: &Path) -> io::Result<()> {
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

#[cfg(windows)]
pub(crate) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
