// 把 assets/app.ico 编译进 exe 资源（资源管理器文件图标 / 任务栏）
fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/app.ico")
            .compile()
            .expect("编译 Windows 图标资源失败（需要 rc.exe，请安装 Windows SDK）");
    }
}
