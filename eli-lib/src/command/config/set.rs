use super::{
    HOMEPAGE_HIGHER_THAN, boolean_config, invalid_key, parse_bootdisk_version, unavailable,
};
use crate::Ctx;
use regex::Regex;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HOMEPAGE_PATTERN: OnceLock<Regex> = OnceLock::new();

/// 设置配置项并返回被修改的目标路径。
pub fn set(ctx: &Ctx, key: &str, value: &str) -> io::Result<PathBuf> {
    let bootdisk = ctx.bootdisk_for_destructive_operation()?;
    set_in_edgeless_dir(
        &bootdisk.mount_point.join("Edgeless"),
        &bootdisk.version,
        key,
        value,
    )
}

fn set_in_edgeless_dir(
    edgeless_dir: &Path,
    version: &str,
    key: &str,
    value: &str,
) -> io::Result<PathBuf> {
    let config_dir = edgeless_dir.join("Config");
    if let Some(config) = boolean_config(key) {
        let version = parse_bootdisk_version(version)?;
        if !config.is_available_for(version) {
            return Err(unavailable(
                config.key,
                version,
                config.supported_version_range(),
            ));
        }
        let enabled = value.parse::<bool>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "boolean config value must be true or false",
            )
        })?;
        let marker = config_dir.join(config.key);
        set_marker(&marker, enabled)?;
        return Ok(marker);
    }
    if key.eq_ignore_ascii_case("wallpaper") {
        return set_wallpaper(edgeless_dir, Path::new(value));
    }
    if key.eq_ignore_ascii_case("resolution") {
        validate_resolution(value)?;
        let path = config_dir.join("分辨率.txt");
        replace_file(&path, value.as_bytes())?;
        return Ok(path);
    }
    if key.eq_ignore_ascii_case("homepage") {
        let version = parse_bootdisk_version(version)?;
        if version <= HOMEPAGE_HIGHER_THAN {
            return Err(unavailable(
                "homepage",
                version,
                format_args!("> {HOMEPAGE_HIGHER_THAN}"),
            ));
        }
        let path = config_dir.join("HomePage.txt");
        replace_file(&path, normalize_homepage(value)?.as_bytes())?;
        return Ok(path);
    }
    Err(invalid_key(key))
}

fn set_marker(path: &Path, enabled: bool) -> io::Result<()> {
    if enabled {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        match fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "config marker is not empty and will not be removed: {}",
                    path.display()
                ),
            )),
            Err(error) => Err(error),
        }
    }
}

fn set_wallpaper(edgeless_dir: &Path, source: &Path) -> io::Result<PathBuf> {
    if !source.is_file() || !has_jpg_extension(source) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "wallpaper must be an existing .jpg file: {}",
                source.display()
            ),
        ));
    }
    let destination = edgeless_dir.join("wp.jpg");
    copy_jpeg_atomically(source, &destination)?;
    Ok(destination)
}

fn has_jpg_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
}

fn validate_resolution(value: &str) -> io::Result<()> {
    if value == "DisableAutoSuit" {
        return Ok(());
    }
    let fields = value.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err(invalid_resolution());
    }
    let width = fields[0]
        .strip_prefix('w')
        .and_then(|number| number.parse::<u16>().ok());
    let height = fields[1]
        .strip_prefix('h')
        .and_then(|number| number.parse::<u16>().ok());
    let bit = fields[2]
        .strip_prefix('b')
        .and_then(|number| number.parse::<u8>().ok());
    let fps = fields[3]
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok());
    let valid_size = matches!(
        width.zip(height),
        Some(
            (1920, 1080)
                | (1680, 1050)
                | (1600, 900)
                | (1440, 900)
                | (1400, 1050)
                | (1366, 768)
                | (1360, 768)
                | (1280, 1024)
                | (1280, 960)
                | (1280, 800)
                | (1280, 768)
                | (1280, 720)
                | (1280, 600)
                | (1152, 864)
                | (1024, 768)
                | (800, 600)
        )
    );
    if valid_size && matches!(bit, Some(16 | 32)) && matches!(fps, Some(30 | 60)) {
        Ok(())
    } else {
        Err(invalid_resolution())
    }
}

fn invalid_resolution() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "resolution must be DisableAutoSuit or `w<width> h<height> b<16|32> f<30|60>` using a supported resolution",
    )
}

fn normalize_homepage(value: &str) -> io::Result<String> {
    if value == "Disable" {
        return Ok(value.to_owned());
    }
    let value = if value.starts_with("http") {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    let pattern = HOMEPAGE_PATTERN.get_or_init(|| {
        Regex::new(r#"^((https|http|ftp|rtsp|mms)?://)?(([0-9a-z_!~*'().&=+$%-]+: )?[0-9a-z_!~*'().&=+$%-]+@)?(([0-9]{1,3}\.){3}[0-9]{1,3}|([0-9a-z_!~*'()-]+\.)*([0-9a-z][0-9a-z-]{0,61})?[0-9a-z]\.[a-z]{2,6})(:[0-9]{1,4})?((/?)|(/[0-9a-z_!~*'().;?:@&=+$,%#-]+)+/?)$"#)
            .expect("homepage validation pattern is valid")
    });
    if !pattern.is_match(&value) {
        return Err(invalid_homepage());
    }
    Ok(value)
}

fn invalid_homepage() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "homepage must be Disable or a valid HTTP(S) URL",
    )
}

fn copy_jpeg_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    let temporary = temporary_path(destination)?;
    fs::create_dir_all(temporary.parent().unwrap())?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        io::copy(&mut BufReader::new(File::open(source)?), &mut output)?;
        output.sync_all()?;
        validate_jpeg(&temporary)?;
        replace_path(&temporary, destination)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// 校验暂存文件，以保证实际发布的内容就是已经验证过的 JPEG。
fn validate_jpeg(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 3];
    if file.read_exact(&mut header).is_err() || header != [0xff, 0xd8, 0xff] {
        return Err(invalid_jpeg(path));
    }
    let length = file.metadata()?.len();
    if length < 4 {
        return Err(invalid_jpeg(path));
    }
    file.seek(SeekFrom::End(-2))?;
    let mut trailer = [0_u8; 2];
    if file.read_exact(&mut trailer).is_err() || trailer != [0xff, 0xd9] {
        return Err(invalid_jpeg(path));
    }
    Ok(())
}

fn invalid_jpeg(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("wallpaper is not a JPEG file: {}", path.display()),
    )
}

fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = temporary_path(path)?;
    fs::create_dir_all(temporary.parent().unwrap())?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        replace_path(&temporary, path)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
    })?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".{}-eli-{}-{sequence}.tmp",
        path.file_name()
            .unwrap_or_else(|| OsStr::new("config"))
            .to_string_lossy(),
        std::process::id()
    )))
}

#[cfg(windows)]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_path(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-lib-config-set-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn changes_boolean_marker_without_removing_nonempty_data() {
        let root = test_root();
        let marker = set_in_edgeless_dir(
            &root,
            "Edgeless_Beta_Ofial_4.1.0_2",
            "DisablePinBrowsers",
            "true",
        )
        .unwrap();
        assert!(marker.is_dir());
        set_in_edgeless_dir(
            &root,
            "Edgeless_Beta_Ofial_4.1.0_2",
            "DisablePinBrowsers",
            "false",
        )
        .unwrap();
        fs::create_dir_all(&marker).unwrap();
        fs::write(marker.join("keep"), "data").unwrap();
        assert!(
            set_in_edgeless_dir(
                &root,
                "Edgeless_Beta_Ofial_4.1.0_2",
                "DisablePinBrowsers",
                "false"
            )
            .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_out_of_window_and_invalid_values() {
        assert_eq!(
            set_in_edgeless_dir(
                &test_root(),
                "Edgeless_Beta_Ofial_4.1.0_2",
                "Developer",
                "true"
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Unsupported
        );
        assert!(validate_resolution("w1920 h1080 b24 f60").is_err());
        assert_eq!(
            normalize_homepage("example.com/path").unwrap(),
            "http://example.com/path"
        );
        assert!(normalize_homepage("http://example.com:80:90/path").is_err());
        assert!(normalize_homepage("http://EXAMPLE.COM").is_err());
        assert_eq!(
            normalize_homepage("http://foo_bar.example.com").unwrap(),
            "http://foo_bar.example.com"
        );
    }

    #[test]
    fn replaces_wallpaper_only_with_a_jpeg() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let image = root.join("source.jpg");
        fs::write(&image, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        assert_eq!(
            fs::read(set_wallpaper(&root, &image).unwrap()).unwrap(),
            [0xff, 0xd8, 0xff, 0xd9]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_jpg_extension_without_jpeg_contents() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let image = root.join("source.jpg");
        fs::write(&image, b"not a jpeg").unwrap();
        assert!(set_wallpaper(&root, &image).is_err());
        assert!(!root.join("wp.jpg").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wallpaper_and_resolution_do_not_require_a_version_identifier() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let image = root.join("source.jpg");
        fs::write(&image, [0xff, 0xd8, 0xff, 0xd9]).unwrap();
        set_in_edgeless_dir(
            &root,
            "legacy-version",
            "wallpaper",
            image.to_str().unwrap(),
        )
        .unwrap();
        set_in_edgeless_dir(&root, "legacy-version", "resolution", "DisableAutoSuit").unwrap();
        assert!(root.join("wp.jpg").is_file());
        assert_eq!(
            fs::read_to_string(root.join("Config").join("分辨率.txt")).unwrap(),
            "DisableAutoSuit"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_writes_leave_one_complete_configuration_file() {
        let root = test_root();
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Config").join("HomePage.txt");
        let values = [
            "http://one.example.com",
            "http://two.example.com",
            "http://three.example.com",
            "Disable",
        ];
        let workers = values.into_iter().map(|value| {
            let path = path.clone();
            std::thread::spawn(move || replace_file(&path, value.as_bytes()))
        });
        for worker in workers {
            worker.join().unwrap().unwrap();
        }

        let content = fs::read_to_string(&path).unwrap();
        assert!(values.contains(&content.as_str()));
        let names = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!names.iter().any(|name| name.ends_with(".tmp")));
        fs::remove_dir_all(root).unwrap();
    }
}
