//! Diagnostic: print what discovery finds on this machine.
//!
//! Run explicitly (it depends on a local WeChat install):
//!   cargo test --test discovery -- --ignored --nocapture

use wechat_devify_bridge_lib::wechat::discover;

#[test]
#[ignore = "依赖本机微信安装，需显式运行"]
fn print_local_inventory() {
    let scan = discover::scan();
    println!("扫描根目录:");
    for root in &scan.roots {
        println!("  {root}");
    }
    println!("账号数: {}", scan.accounts.len());
    for account in &scan.accounts {
        println!(
            "\n账号 {} ({}) — {} 个库，加密 {}，合计 {:.1} MB",
            account.id,
            account.dir,
            account.db_count,
            account.encrypted_count,
            account.total_bytes as f64 / 1024.0 / 1024.0
        );
        for db in &account.dbs {
            println!(
                "  {:<32} {:>9.1} MB  encrypted={} wal={} salt={}",
                db.rel,
                db.size as f64 / 1024.0 / 1024.0,
                db.encrypted,
                db.has_wal,
                db.salt.as_deref().unwrap_or("-")
            );
        }
    }
    if !scan.skipped.is_empty() {
        println!("\n跳过: {:#?}", scan.skipped);
    }
}
