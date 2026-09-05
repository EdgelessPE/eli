mod download;
mod version;

pub use download::{DownloadResult, download};
pub use version::{bootdisk, current, latest};
