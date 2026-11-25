//! Receiver-led Skills installation. No guessed host directory or token in copied text.
use std::path::Path;

pub fn build(kit: &Path, data: &Path, endpoint: &str) -> String {
    let name = crate::build_flavor::SKILL_NAME;
    let source = kit.join(name).join("SKILL.md");
    let command = format!("{} --app-data {} --endpoint {}", super::sh_quote(&data.join("bin").join(super::CLI_NAME).to_string_lossy()), super::sh_quote(&data.to_string_lossy()), super::sh_quote(endpoint));
    format!("请为你自己安装 AIMyChats 的 {name} 技能，用于通过本机 AIMyChats 读取获准的聊天记录、当前空间已有待办和已生成报告。\n\n\
1. 确认你与 AIMyChats 运行在同一台电脑。识别你实际使用的客户端或 CLI，并查明它支持的 Skills 目录；不要把其他智能体的目录当作自己的目录。若不支持本机 Skills 或目录无法确定，停止并说明原因。\n\
2. 安装源已由 AIMyChats 准备在本机：{source}。读取这个文件，将它原样安装到你确认的 Skills 目录下的 {name}/SKILL.md。先说明安装位置；若目标已有不同内容，先保留备份，不要丢弃自定义内容。无需下载、编译或覆盖全局命令。不要覆盖其他 AIMyChats 版本的技能。\n\
3. 保持 AIMyChats 打开并登录。在当前智能体运行下面的健康检查。成功后，用同一入口运行 accounts --json。有可读会话时，用返回的账号 ID 执行 sessions --account ID --limit 1 --offset 0 --json。安装测试只检查连接和会话列表，凭据由工具自动读取。\n\n\
```sh\n{command} health --json\n```\n\n\
4. 若出现 permission_denied、Operation not permitted 或宿主阻止命令，停止读取。仅当宿主提供官方本机授权入口时，提示用户授权该本机连接，再在同一宿主重试。无法确认时报告当前模式未验证。其他错误按 Skill 的首次使用指引处理。请勿更改全局权限或将服务开放到公网。\n\n\
5. 分别汇报文件安装、Skill 识别、健康检查、账号与会话查询的结果。若没有获准会话，说明连接已通但可读范围为空。只汇报状态和诊断代码，省略凭据、账号 ID、会话名称和聊天内容。\n", source=source.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_is_bound_to_this_app_and_leaves_host_discovery_to_receiver() {
        let data = Path::new("/tmp/it's app");
        let text = build(&data.join("agent-access-kit"), data, "http://127.0.0.1:18766");
        assert!(text.contains(&format!("{}/SKILL.md", crate::build_flavor::SKILL_NAME)));
        assert!(text.contains("识别你实际使用的客户端或 CLI"));
        assert!(!text.contains(".codex/skills"));
        assert!(text.contains("先保留备份"));
        assert!(text.contains("it'\\''s app"));
        assert!(text.contains("--endpoint 'http://127.0.0.1:18766'"));
        assert!(text.contains("health --json"));
        assert!(text.contains("sessions --account ID --limit 1 --offset 0 --json"));
        assert!(text.contains("Operation not permitted"));
        assert!(!text.contains("agent-token"));
    }
}
