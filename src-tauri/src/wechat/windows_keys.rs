//! Windows-specific WeChat key acquisition.
//!
//! The SQLCipher/page verification path stays shared with macOS. This module only discovers
//! the running Weixin.exe process and scans readable private memory for the legacy 4.x
//! `x'<64 hex key><32 hex salt>'` keyspec. Newer WeChat builds may keep a codec context instead;
//! those builds should fall back to importing a trusted key manifest until a version profile is
//! added. Candidate material is always verified against local database page HMAC before import.

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::c_void;
    use std::process::Command;

    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_GUARD: u32 = 0x100;
    const PAGE_NOACCESS: u32 = 0x01;
    const READABLE: [u32; 6] = [0x02, 0x04, 0x08, 0x20, 0x40, 0x80];
    const MAX_ADDRESS: usize = 0x0000_7fff_ffff_ffff;

    #[repr(C)]
    struct MemoryBasicInformation {
        base_address: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn VirtualQueryEx(
            process: *mut c_void,
            address: *const c_void,
            buffer: *mut MemoryBasicInformation,
            length: usize,
        ) -> usize;
        fn ReadProcessMemory(
            process: *mut c_void,
            address: *const c_void,
            buffer: *mut c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
    }

    fn process_ids(name: &str) -> Vec<u32> {
        let Ok(output) = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"])
            .output()
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.trim_matches('\r').split(',').collect();
                fields.get(1)?.trim_matches('"').parse().ok()
            })
            .collect()
    }

    fn readable(protect: u32) -> bool {
        protect & PAGE_GUARD == 0
            && protect & 0xff != PAGE_NOACCESS
            && READABLE.contains(&(protect & 0xff))
    }

    fn scan_process(pid: u32) -> Vec<String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut address = 0usize;
        while address < MAX_ADDRESS {
            let mut info = MemoryBasicInformation {
                base_address: std::ptr::null_mut(),
                allocation_base: std::ptr::null_mut(),
                allocation_protect: 0,
                partition_id: 0,
                region_size: 0,
                state: 0,
                protect: 0,
                kind: 0,
            };
            let queried = unsafe {
                VirtualQueryEx(
                    handle,
                    address as *const c_void,
                    &mut info,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            if queried == 0 || info.region_size == 0 {
                break;
            }
            let next = (info.base_address as usize).saturating_add(info.region_size);
            if info.state == MEM_COMMIT && readable(info.protect) {
                let mut offset = 0usize;
                while offset < info.region_size {
                    let size = (info.region_size - offset).min(4 * 1024 * 1024);
                    let mut bytes = vec![0u8; size];
                    let mut read = 0usize;
                    let ok = unsafe {
                        ReadProcessMemory(
                            handle,
                            (info.base_address as usize + offset) as *const c_void,
                            bytes.as_mut_ptr() as *mut c_void,
                            size,
                            &mut read,
                        )
                    };
                    if ok != 0 && read > 0 {
                        let data = &bytes[..read];
                        let mut index = 0usize;
                        while let Some(start) = data[index..].iter().position(|byte| *byte == b'x')
                        {
                            let start = index + start;
                            if start + 99 <= data.len()
                                && data[start + 1] == b'\''
                                && data[start + 98] == b'\''
                                && data[start + 2..start + 98]
                                    .iter()
                                    .all(|byte| byte.is_ascii_hexdigit())
                            {
                                found.push(
                                    String::from_utf8_lossy(&data[start..start + 99]).to_string(),
                                );
                            }
                            index = start.saturating_add(1);
                            if index >= data.len() {
                                break;
                            }
                        }
                    }
                    offset = offset.saturating_add(size);
                }
            }
            if next <= address {
                break;
            }
            address = next;
        }
        unsafe {
            CloseHandle(handle);
        }
        found.sort();
        found.dedup();
        found
    }

    pub fn scan() -> Result<Vec<String>, String> {
        let mut found = Vec::new();
        for process_name in ["Weixin.exe", "WeChat.exe"] {
            for pid in process_ids(process_name) {
                found.extend(scan_process(pid));
            }
        }
        found.sort();
        found.dedup();
        if found.is_empty() {
            Err("没有在运行中的 Weixin.exe 内存中找到旧版 x'<密钥><salt>' 密钥格式。请确认微信已登录；4.1.11+ 可能需要对应版本的 codec context 提取器，或导入可信密钥 JSON。".into())
        } else {
            Ok(found)
        }
    }
}

#[cfg(target_os = "windows")]
pub fn scan() -> Result<Vec<String>, String> {
    imp::scan()
}

#[cfg(not(target_os = "windows"))]
pub fn scan() -> Result<Vec<String>, String> {
    Err("Windows 密钥扫描器只能在 Windows 上运行。".into())
}
