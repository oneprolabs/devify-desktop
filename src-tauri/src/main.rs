#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    if std::env::args().any(|arg| arg == "--build-info") {
        println!("{}", wechat_devify_bridge_lib::build_flavor::metadata());
        return;
    }
    wechat_devify_bridge_lib::run()
}
