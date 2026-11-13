//! Developer probe: run the chat-path decryption against the real local account and print
//! every outcome (rel, status, error) plus the key store state, to find out which database
//! the app reports as "no-key".
//!
//! Usage:
//!   cargo run --quiet --example decrypt_probe

use wechat_devify_bridge_lib::wechat::{decrypt, discover, keys};

fn main() {
    let app_data = std::path::PathBuf::from(std::env::var("HOME").expect("HOME"))
        .join("Library/Application Support/local.wechat.devifybridge");

    let store = keys::load(&keys::store_path(&app_data));
    println!("key store entries: {}", store.keys.len());

    let scan = discover::scan();
    println!("accounts: {}", scan.accounts.len());
    let Some(account) = scan.accounts.first() else {
        println!("no account found (full disk access?)");
        return;
    };
    println!("account id: {}", account.id);
    println!("dbs: {}", account.dbs.len());

    for db in &account.dbs {
        let key = keys::lookup(&store, &account.id, &db.rel);
        let chat = db.rel == "session/session.db"
            || db.rel.starts_with("message/message_")
            || db.rel.starts_with("head_image/")
            || db.rel == "contact/contact.db";
        println!(
            "  {:<42} size={:<10} enc={:<5} key={} chat={} salt={:?}",
            db.rel,
            db.size,
            db.encrypted,
            if key.is_some() { "yes" } else { "NO" },
            chat,
            db.salt.as_deref().map(|s| &s[..8.min(s.len())])
        );
        if let (Some(entry), Some(salt)) = (key, db.salt.as_deref()) {
            if entry.salt != salt {
                println!("      salt mismatch: stored {} vs current {}", entry.salt, salt);
            }
        }
    }

    let outcomes = decrypt::decrypt_account(&app_data, account, &store, true, true);
    println!("\noutcomes:");
    for outcome in &outcomes {
        println!(
            "  {:<42} {:<8} {:?}",
            outcome.rel, outcome.status, outcome.error
        );
    }
}
