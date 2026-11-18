//! Developer probe: print what the WhatsApp reader sees on this machine.
//!
//! Usage:
//!   cargo run --quiet --example whatsapp_probe [keyword]
//!
//! The live database is only read; everything goes through a copy under a temporary
//! directory that is removed on exit.

use std::time::{SystemTime, UNIX_EPOCH};

use wechat_devify_bridge_lib::whatsapp;

fn main() {
    let temp = std::env::temp_dir().join(format!(
        "whatsapp-probe-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
    ));
    std::fs::create_dir_all(&temp).unwrap();

    let status = whatsapp::status(&temp);
    println!("installed: {}", status.installed);
    println!("app: {:?}", status.app_path);
    println!("version: {:?}", status.version);
    println!("account: {:?} ({:?})", status.account, status.name);
    println!("data: {:?}", status.data_dir);
    println!("sessions: {} · messages: {}", status.sessions, status.messages);
    if let Some(error) = status.error.as_deref() {
        println!("error: {error}");
    }

    let sessions = match whatsapp::sessions(&temp) {
        Ok(list) => list,
        Err(error) => {
            eprintln!("读取会话失败：{error}");
            std::process::exit(1);
        }
    };
    for session in &sessions {
        println!(
            "- {} · {} · 群聊={} · 成员={:?} · {} 条未读 · 最新 {:?}",
            session.username,
            session.display,
            session.is_group,
            session.member_count,
            session.unread.unwrap_or_default(),
            session.last_time
        );
    }

    if let Some(first) = sessions.first() {
        println!("\n最近会话 {} 的消息：", first.display);
        match whatsapp::messages(&temp, &first.username, 20, None) {
            Ok(list) => {
                for message in list.iter().rev() {
                    println!(
                        "  [{}] {} {}",
                        message.time,
                        message.sender_name.as_deref().unwrap_or_else(|| if message.is_self { "我" } else { "?" }),
                        message.text
                    );
                }
            }
            Err(error) => println!("  读取失败：{error}"),
        }
    }

    let names: Vec<String> = sessions.iter().map(|session| session.username.clone()).collect();
    match whatsapp::upload_conversations(&temp, &names, None, None, &Default::default()) {
        Ok(list) => {
            println!("\n上传打包预览：");
            for conversation in list {
                println!(
                    "  {} · {} 条 · 首键 {:?} · 末键 {:?}",
                    conversation.session.display,
                    conversation.entries.len(),
                    conversation.entries.first().map(|(key, _)| key.clone()),
                    conversation.entries.last().map(|(key, _)| key.clone()),
                );
            }
        }
        Err(error) => println!("  打包预览失败：{error}"),
    }

    let keyword = std::env::args().nth(1).unwrap_or_else(|| "hello".to_string());
    match whatsapp::search(&temp, &keyword, 50, None) {
        Ok(report) => {
            println!("\n搜索「{}」：扫描 {} 个会话，跳过 {} 个，命中 {} 个会话", report.keyword, report.scanned, report.skipped, report.hits.len());
            for hit in report.hits {
                println!("  {} · {} 条", hit.display, hit.hits);
            }
        }
        Err(error) => println!("\n搜索失败：{error}"),
    }

    let _ = std::fs::remove_dir_all(&temp);
}
