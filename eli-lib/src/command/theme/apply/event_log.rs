// 主题应用追加事件日志。
//
// 日志位于当前 PE 会话主题目录中，并且只在实际提交可应用组件时创建。
// named mutex 覆盖整个提交阶段，因此单次写入不会与另一个 theme apply 交错。

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::ThemePaths;

pub struct ThemeEventLog {
    file: File,
    transaction_id: String,
    source: String,
}

impl ThemeEventLog {
    pub fn open(paths: &ThemePaths, source: &Path) -> io::Result<Self> {
        let session_root = paths.staging_root.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "theme staging directory has no session parent: {}",
                    paths.staging_root.display()
                ),
            )
        })?;
        let log_path = session_root.join("theme-apply.log");
        super::transaction::ensure_safe_publish_path(session_root, &log_path)?;
        std::fs::create_dir_all(session_root)?;
        super::transaction::ensure_safe_publish_path(session_root, &log_path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        Ok(Self {
            file,
            transaction_id: super::transaction::unique_transaction_id(),
            source: source.to_string_lossy().into_owned(),
        })
    }

    pub fn record(
        &mut self,
        component: &str,
        phase: &str,
        result: &str,
        windows_error_code: Option<i32>,
        detail: Option<&str>,
    ) -> io::Result<()> {
        let error_code = windows_error_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_owned());
        let detail = detail.unwrap_or("");
        writeln!(
            self.file,
            "transaction_id={}\tsource={}\tcomponent={}\tphase={}\tresult={}\twindows_error_code={}\tdetail={}",
            escape_field(&self.transaction_id),
            escape_field(&self.source),
            escape_field(component),
            escape_field(phase),
            escape_field(result),
            error_code,
            escape_field(detail),
        )?;
        self.file.flush()
    }
}

fn escape_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::theme::apply::test_support::test_root;
    use std::path::PathBuf;

    #[test]
    fn escapes_each_record_to_exactly_one_line() {
        assert_eq!(escape_field("a\\b\tc\r\nd"), "a\\\\b\\tc\\r\\nd");
    }

    #[test]
    fn writes_all_required_event_fields_and_the_windows_error_code() {
        let root = test_root("event-log");
        let paths = ThemePaths {
            system_root: root.join("Windows"),
            staging_root: root.join("Users/Theme/eli/staging"),
            wallpaper_dir: root.join("Users/Theme/eli/wallpaper"),
            icon_root: root.join("Users/Icon"),
            cursor_root: root.join("Windows/Cursors/Edgeless"),
            desktop_roots: Vec::new(),
            icon_cache_dir: root.join("Cache"),
        };
        let source = PathBuf::from("D:\\Themes\\Sample.eth");
        let mut log = ThemeEventLog::open(&paths, &source).unwrap();

        log.record(
            "MouseStyle.ems",
            "commit",
            "failed",
            Some(5),
            Some("access denied"),
        )
        .unwrap();
        drop(log);

        let contents =
            std::fs::read_to_string(paths.staging_root.parent().unwrap().join("theme-apply.log"))
                .unwrap();
        assert!(contents.contains("transaction_id=t-"));
        assert!(contents.contains("source=D:\\\\Themes\\\\Sample.eth"));
        assert!(contents.contains("component=MouseStyle.ems"));
        assert!(contents.contains("phase=commit"));
        assert!(contents.contains("result=failed"));
        assert!(contents.contains("windows_error_code=5"));
        assert!(contents.contains("detail=access denied"));
        assert_eq!(contents.lines().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
