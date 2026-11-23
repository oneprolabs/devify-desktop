//! Direct, repeatable installation of the bundled local-only access kit.
use super::{AccessTarget, SKILLS};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static INSTALL: Mutex<()> = Mutex::new(());

fn unique_suffix() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("创建安装文件标识失败：{e}"))?;
    Ok(hex::encode(bytes))
}

#[derive(Serialize)]
pub struct InstallResult {
    pub tool: String,
    pub installed: Vec<String>,
    pub skills_dir: String,
    pub legacy_preserved: Vec<String>,
}

// Same-directory temporary file + rename: never expose a partially written Skill/CLI.
fn write_owned(path: &Path, bytes: &[u8], executable: bool) -> Result<(), String> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("安装目标不是普通文件，请在高级配置中选择其他目录。".into());
        }
    }
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let parent = path.parent().ok_or("安装目录无效。")?;
    fs::create_dir_all(parent).map_err(|e| format!("创建安装目录失败：{e}"))?;
    let suffix = unique_suffix()?;
    let temp = parent.join(format!(".aimychats-{suffix}.tmp"));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("创建安装文件失败：{e}"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("写入安装文件失败：{e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))
            .map_err(|e| format!("设置安装权限失败：{e}"))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        if path.exists() && !executable {
            fs::copy(
                path,
                parent.join(format!(
                    "{}.bak-{suffix}",
                    path.file_name().unwrap().to_string_lossy()
                )),
            )
            .map_err(|e| format!("备份原文件失败：{e}"))?;
        }
        fs::rename(&temp, path).map_err(|e| format!("更新安装文件失败：{e}"))
    })();
    let _ = fs::remove_file(&temp);
    result
}

pub fn install(
    data: &Path,
    target: &AccessTarget,
    cli: &Path,
    endpoint: &str,
) -> Result<InstallResult, String> {
    let _guard = INSTALL
        .try_lock()
        .map_err(|_| "已有 Skills 安装正在执行，请稍后重试。")?;
    let skills =
        super::expand_path(&target.skills_dir).ok_or("Skills 目录必须是绝对路径或以 ~/ 开头。")?;
    // Own a private CLI destination, so a user's unrelated PATH executable is never overwritten.
    let installed_cli = data.join("bin").join(super::CLI_NAME);
    let bytes = fs::read(cli).map_err(|e| format!("读取本机工具失败：{e}"))?;
    write_owned(&installed_cli, &bytes, true)?;
    let command = format!(
        "{} --app-data {} --endpoint {}",
        super::sh_quote(&installed_cli.to_string_lossy()),
        super::sh_quote(&data.to_string_lossy()),
        super::sh_quote(endpoint)
    );
    for skill in SKILLS {
        // Insert after YAML, preserving the host's mandatory frontmatter parsing.
        let end = skill
            .content
            .strip_prefix("---\n")
            .and_then(|body| body.find("\n---\n").map(|n| n + 9))
            .ok_or("内置 Skill 格式无效。")?;
        let content = format!("{}\n<!-- AIMyChats local access kit -->\n本机读取入口：所有下文的 `wxq` 命令请替换为 `{command}`。已由 AIMyChats 安装，无需下载或构建。保持 AIMyChats 打开并登录；Token 由 CLI 自动读取，不要回显或复制。只读取应用允许访问的会话。\n\n{}", &skill.content[..end], &skill.content[end..]);
        write_owned(
            &skills.join(skill.name).join("SKILL.md"),
            content.as_bytes(),
            false,
        )?;
    }
    // Retire only app-managed legacy Skills, after all replacements are installed.
    // Move them outside hosts' watched Skills directories; never discard user files.
    let mut legacy_preserved = Vec::new();
    for legacy_name in SKILLS.iter().flat_map(|skill| skill.legacy_names.iter()).filter(|_| !crate::build_flavor::DEVELOPMENT) {
        let old = skills.join(legacy_name);
        let Ok(meta) = fs::symlink_metadata(&old) else {
            continue;
        };
        let doc = old.join("SKILL.md");
        let owned = meta.is_dir()
            && !meta.file_type().is_symlink()
            && fs::symlink_metadata(&doc).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
            && fs::read_to_string(&doc)
                .is_ok_and(|text| text.contains("<!-- AIMyChats local access kit -->"));
        let backup = data.join("legacy-skill-backups");
        let moved = owned
            && fs::create_dir_all(&backup).is_ok()
            && fs::rename(
                &old,
                backup.join(format!("{}-{}", legacy_name, unique_suffix()?)),
            )
            .is_ok();
        if !moved {
            legacy_preserved.push(legacy_name.to_string());
        }
    }
    Ok(InstallResult {
        tool: target.id.clone(),
        installed: SKILLS.iter().map(|skill| skill.name.into()).collect(),
        skills_dir: skills.to_string_lossy().to_string(),
        legacy_preserved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_is_repeatable_and_preserves_unrelated_files() {
        let root =
            std::env::temp_dir().join(format!("aimychats-install-{}", unique_suffix().unwrap()));
        fs::create_dir_all(&root).unwrap();
        let cli = root.join("source-cli");
        fs::write(&cli, b"test cli").unwrap();
        let skills = root.join("skills");
        fs::create_dir_all(&skills).unwrap();
        fs::write(skills.join("unrelated"), b"keep").unwrap();
        if crate::build_flavor::DEVELOPMENT {
            fs::create_dir_all(skills.join("aimychats")).unwrap();
            fs::write(skills.join("aimychats/SKILL.md"), b"production skill").unwrap();
        }
        let legacy = skills.join(SKILLS[0].legacy_names[0]);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(
            legacy.join("SKILL.md"),
            b"<!-- AIMyChats local access kit -->\nlegacy",
        )
        .unwrap();
        let custom = skills.join(SKILLS[0].legacy_names[1]);
        fs::create_dir_all(&custom).unwrap();
        fs::write(custom.join("SKILL.md"), b"my custom skill").unwrap();
        let target = AccessTarget {
            id: "codex".into(),
            name: "Codex".into(),
            available: true,
            command: None,
            skills_dir: skills.to_string_lossy().into(),
            installed: vec![],
        };
        let data = root.join("app");
        for _ in 0..2 {
            assert_eq!(
                install(&data, &target, &cli, "http://127.0.0.1:18765")
                    .unwrap()
                    .installed
                    .len(),
                1
            );
        }
        let text = fs::read_to_string(skills.join(SKILLS[0].name).join("SKILL.md")).unwrap();
        if crate::build_flavor::DEVELOPMENT {
            assert!(legacy.exists(), "development must not retire production/legacy Skills");
            assert_eq!(fs::read(skills.join("aimychats/SKILL.md")).unwrap(), b"production skill");
            assert!(!data.join("legacy-skill-backups").exists());
            assert!(text.starts_with("---\nname: aimychats-dev\n"));
            assert!(text.contains("--app-data"));
        } else {
            assert!(!legacy.exists(), "managed legacy Skills are no longer discovered twice");
            assert_eq!(fs::read_dir(data.join("legacy-skill-backups")).unwrap().count(), 1);
            assert_eq!(install(&data, &target, &cli, "http://127.0.0.1:18765").unwrap().legacy_preserved,
                vec![SKILLS[0].legacy_names[1]]);
        }
        assert_eq!(fs::read(custom.join("SKILL.md")).unwrap(), b"my custom skill");
        assert!(text.starts_with("---\nname:"));
        assert!(text.contains("--endpoint"));
        assert_eq!(fs::read(skills.join("unrelated")).unwrap(), b"keep");
        assert_eq!(
            fs::read_dir(skills.join(SKILLS[0].name)).unwrap().count(),
            1
        );
        fs::write(
            skills.join(SKILLS[0].name).join("SKILL.md"),
            b"custom prior skill",
        )
        .unwrap();
        install(&data, &target, &cli, "http://127.0.0.1:18765").unwrap();
        assert_eq!(
            fs::read_dir(skills.join(SKILLS[0].name)).unwrap().count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn installer_rejects_linked_targets() {
        let root =
            std::env::temp_dir().join(format!("aimychats-link-{}", unique_suffix().unwrap()));
        fs::create_dir_all(&root).unwrap();
        let victim = root.join("victim");
        fs::write(&victim, b"keep").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(write_owned(&link, b"replace", false).is_err());
        assert_eq!(fs::read(victim).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }
}
