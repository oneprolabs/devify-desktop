//! WeChat database key scanner (macOS).
//!
//! Reads the running WeChat process memory and extracts SQLCipher key material in the
//! `x'<64 hex key><32 hex salt>'` form, then verifies every candidate against the first
//! page HMAC of the databases found on disk. Only verified keys are written.
//!
//! Requirements (both are mandatory):
//!   1. run as root (`sudo`), and
//!   2. the system must allow a debugger to inspect the target process. Probe it first with
//!      `sudo lldb -p $(pgrep -x WeChat) -o "detach" -o "quit"`.
//!      Stock WeChat 4.1.15 refuses this even as root, so on an official install this tool
//!      cannot work; importing keys from a trusted external tool is the supported path.
//!
//! Do NOT re-sign WeChat to make this work: ad-hoc `--deep` signing on 4.1.15 strips its
//! sandbox/App Group entitlements and the app then exits at launch. See README.
//!
//! Usage: sudo wechat-key-scan [--out <path>] [--pid <pid>] [--process <name>]
//!
//! The tool never prints key material; it prints per-database verification results.

#[cfg(target_os = "macos")]
fn main() {
    if let Err(error) = imp::run() {
        eprintln!("失败：{error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("wechat-key-scan 目前只支持 macOS。");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Read;
    use std::mem;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use mach2::kern_return::KERN_SUCCESS;
    use mach2::mach_port::mach_port_deallocate;
    use mach2::mach_types::vm_task_entry_t;
    use mach2::traps::{mach_task_self, task_for_pid};
    use mach2::vm::{mach_vm_deallocate, mach_vm_read, mach_vm_region};
    use mach2::vm_prot::{VM_PROT_READ, VM_PROT_WRITE};
    use mach2::vm_region::{vm_region_basic_info_64, VM_REGION_BASIC_INFO_64};
    use mach2::vm_types::{mach_vm_address_t, mach_vm_size_t, vm_offset_t};

    use wechat_devify_bridge_lib::wechat::crypto;
    use wechat_devify_bridge_lib::wechat::discover;
    use wechat_devify_bridge_lib::wechat::keys::{self, KeyStore};

    const CHUNK: usize = 4 * 1024 * 1024;
    const OVERLAP: usize = 128;
    const PATTERN_LEN: usize = 99; // x' + 96 hex + '

    struct Args {
        out: PathBuf,
        pid: Option<i32>,
        process: String,
    }

    fn parse_args() -> Result<Args, String> {
        let mut out = PathBuf::from("wechat-keys.json");
        let mut pid = None;
        let mut process = "WeChat".to_string();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--out" => out = PathBuf::from(args.next().ok_or("--out 缺少参数")?),
                "--pid" => {
                    pid = Some(
                        args.next()
                            .ok_or("--pid 缺少参数")?
                            .parse::<i32>()
                            .map_err(|_| "--pid 需要整数")?,
                    )
                }
                "--process" => process = args.next().ok_or("--process 缺少参数")?,
                "--help" | "-h" => {
                    println!("用法: sudo wechat-key-scan [--out <path>] [--pid <pid>] [--process <name>]");
                    std::process::exit(0);
                }
                other => return Err(format!("未知参数 {other}")),
            }
        }
        Ok(Args { out, pid, process })
    }

    fn find_pid(process: &str) -> Result<i32, String> {
        let output = Command::new("pgrep")
            .arg("-x")
            .arg(process)
            .output()
            .map_err(|e| format!("无法执行 pgrep：{e}"))?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.split_whitespace()
            .next()
            .and_then(|pid| pid.parse::<i32>().ok())
            .ok_or_else(|| format!("找不到正在运行的 {process} 进程"))
    }

    struct Candidate {
        key: Vec<u8>,
        salt: Vec<u8>,
    }

    fn scan_memory(pid: i32) -> Result<Vec<Candidate>, String> {
        unsafe {
            let mut task: vm_task_entry_t = 0;
            let kr = task_for_pid(mach_task_self(), pid, &mut task);
            if kr != KERN_SUCCESS {
                return Err(format!(
                    "task_for_pid 失败（kern_return={kr}）：需要 root，且微信必须是非 Hardened Runtime 签名"
                ));
            }

            let mut candidates: BTreeMap<(Vec<u8>, Vec<u8>), ()> = BTreeMap::new();
            let mut address: mach_vm_address_t = 0;

            loop {
                let mut size: mach_vm_size_t = 0;
                let mut info: vm_region_basic_info_64 = mem::zeroed();
                let mut info_count = vm_region_basic_info_64::count();
                let mut object_name = 0;

                let kr = mach_vm_region(
                    task,
                    &mut address,
                    &mut size,
                    VM_REGION_BASIC_INFO_64,
                    (&mut info as *mut vm_region_basic_info_64) as *mut _,
                    &mut info_count,
                    &mut object_name,
                );
                if kr != KERN_SUCCESS || size == 0 {
                    break;
                }
                if object_name != 0 {
                    mach_port_deallocate(mach_task_self(), object_name);
                }

                let readable = info.protection & VM_PROT_READ != 0;
                let writable = info.protection & VM_PROT_WRITE != 0;
                if readable && writable {
                    let mut offset: mach_vm_size_t = 0;
                    while offset < size {
                        let chunk = std::cmp::min(CHUNK as mach_vm_size_t, size - offset);
                        let mut data: vm_offset_t = 0;
                        let mut data_count: u32 = 0;
                        let kr = mach_vm_read(
                            task,
                            address + offset,
                            chunk,
                            &mut data,
                            &mut data_count,
                        );
                        if kr == KERN_SUCCESS && data != 0 {
                            let slice =
                                std::slice::from_raw_parts(data as *const u8, data_count as usize);
                            collect(slice, &mut candidates);
                            mach_vm_deallocate(mach_task_self(), data as mach_vm_address_t, data_count as mach_vm_size_t);
                        }
                        if chunk <= OVERLAP as mach_vm_size_t {
                            break;
                        }
                        offset += chunk - OVERLAP as mach_vm_size_t;
                    }
                }

                address = address.wrapping_add(size);
            }

            mach_port_deallocate(mach_task_self(), task);
            Ok(candidates
                .into_keys()
                .map(|(key, salt)| Candidate { key, salt })
                .collect())
        }
    }

    fn collect(data: &[u8], out: &mut BTreeMap<(Vec<u8>, Vec<u8>), ()>) {
        if data.len() < PATTERN_LEN {
            return;
        }
        let mut index = 0usize;
        while index + PATTERN_LEN <= data.len() {
            if data[index] == b'x' && data[index + 1] == b'\'' {
                let hex_part = &data[index + 2..index + 98];
                if data[index + 98] == b'\'' && hex_part.iter().all(|b| b.is_ascii_hexdigit()) {
                    let text = std::str::from_utf8(hex_part).unwrap_or_default().to_ascii_lowercase();
                    if let (Ok(key), Ok(salt)) = (hex::decode(&text[..64]), hex::decode(&text[64..96])) {
                        out.insert((key, salt), ());
                    }
                    index += 99;
                    continue;
                }
            }
            index += 1;
        }
    }

    fn load_page1(path: &Path) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; crypto::PAGE_SIZE];
        fs::File::open(path).ok()?.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    pub fn run() -> Result<(), String> {
        let args = parse_args()?;
        if unsafe { libc_geteuid() } != 0 {
            return Err("需要 root 权限：请用 sudo 运行。".into());
        }
        let pid = match args.pid {
            Some(pid) => pid,
            None => find_pid(&args.process)?,
        };
        eprintln!("扫描进程 {pid} 的内存…");
        let candidates = scan_memory(pid)?;
        eprintln!("内存中找到 {} 个去重后的密钥候选", candidates.len());
        if candidates.is_empty() {
            return Err(
                "没有找到 x'<key><salt>' 形态的候选：可能是权限不足、微信未打开过目标库、或版本已改为只在内存缓存 passphrase。"
                    .into(),
            );
        }

        let scan = discover::scan();
        if scan.accounts.is_empty() {
            return Err("没有找到本机微信数据目录。".into());
        }

        let mut store = KeyStore::default();
        let mut verified = 0usize;
        let mut unresolved = Vec::new();
        for account in &scan.accounts {
            for db in account.dbs.iter().filter(|d| d.encrypted) {
                let Some(page1) = load_page1(Path::new(&db.path)) else {
                    unresolved.push(format!("{}::{}（无法读取）", account.id, db.rel));
                    continue;
                };
                let Some(salt) = crypto::read_salt(&page1) else {
                    continue;
                };
                let mut hit = false;
                for candidate in &candidates {
                    if candidate.salt.as_slice() != salt {
                        continue;
                    }
                    if crypto::verify_key(&page1, &candidate.key) {
                        keys::store_key(
                            &mut store,
                            &account.id,
                            &db.rel,
                            &candidate.key,
                            &salt,
                            "内存扫描",
                        );
                        verified += 1;
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    unresolved.push(format!("{}::{}", account.id, db.rel));
                }
            }
        }

        keys::save(&args.out, &store)?;
        println!("已验证 {verified} 个数据库密钥，写入 {}", args.out.to_string_lossy());
        if !unresolved.is_empty() {
            println!("未取到密钥的数据库（可能微信尚未打开过，属于正常情况）：");
            for item in unresolved {
                println!("  - {item}");
            }
        }
        Ok(())
    }

    extern "C" {
        #[link_name = "geteuid"]
        fn libc_geteuid() -> u32;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn accepts_the_standard_pattern_and_dedupes() {
            let hex = format!("{}{}", "ab".repeat(32), "cd".repeat(16));
            let text = format!("noisex'{hex}'tail x'{hex}'");
            let mut found = BTreeMap::new();
            collect(text.as_bytes(), &mut found);
            assert_eq!(found.len(), 1, "同一候选应去重");
            let (key, salt) = found.keys().next().unwrap();
            assert_eq!(key.len(), 32);
            assert_eq!(salt.len(), 16);
        }

        #[test]
        fn rejects_non_hex_and_short_patterns() {
            let bad = format!("x'{}'", "zz".repeat(48));
            let mut found = BTreeMap::new();
            collect(bad.as_bytes(), &mut found);
            assert!(found.is_empty());

            let short = format!("x'{}'", "ab".repeat(30));
            let mut found_short = BTreeMap::new();
            collect(short.as_bytes(), &mut found_short);
            assert!(found_short.is_empty());
        }
    }
}
