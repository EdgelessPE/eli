fn main() {
    println!("cargo:rerun-if-changed=../ui");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        slint_build::compile("../ui/plugin/load.slint").expect("无法编译插件加载界面");
    }
}
