use super::Plugin;
use super::package;
use crate::Ctx;
use std::io;
use std::path::Path;

/// 列出选中启动盘中的插件包。
pub fn list(ctx: &Ctx) -> io::Result<Vec<Plugin>> {
    let bootdisk = &ctx.bootdisk()?.selected;
    list_from_resource_dir(&package::resource_dir(&bootdisk.mount_point))
}

fn list_from_resource_dir(resource_dir: &Path) -> io::Result<Vec<Plugin>> {
    let mut packages = package::paths(resource_dir)?;
    packages.sort_unstable();
    packages
        .into_iter()
        .map(|path| package::parse(&path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::plugin::{PluginAttribute, test_resource_dir};
    use std::fs;

    #[test]
    fn lists_plugin_metadata_and_attributes() {
        let resource = test_resource_dir();
        fs::write(resource.join("搜狗拼音_16.4.0.0_Cno（bot）.7zf"), "package").unwrap();
        fs::write(resource.join("工具箱_1.0.0_Edgeless.7zl"), "package").unwrap();

        let plugins = list_from_resource_dir(&resource).unwrap();

        assert_eq!(
            plugins,
            vec![
                Plugin {
                    name: "工具箱".to_owned(),
                    version: "1.0.0".to_owned(),
                    author: "Edgeless".to_owned(),
                    attribute: PluginAttribute::LocalBoost,
                    automatically_built: false,
                },
                Plugin {
                    name: "搜狗拼音".to_owned(),
                    version: "16.4.0.0".to_owned(),
                    author: "Cno".to_owned(),
                    attribute: PluginAttribute::Frozen,
                    automatically_built: true,
                },
            ]
        );
        fs::remove_dir_all(resource).unwrap();
    }
}
