fn main() {
    println!("cargo:rerun-if-changed=../ui");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        slint_build::compile("../ui/app.slint").expect("无法编译 Eli 图形界面");
    }
}
