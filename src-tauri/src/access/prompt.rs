//! Receiver-led Skills installation. No guessed host directory or token in copied text.
use std::path::Path;

pub fn build(kit: &Path, data: &Path, endpoint: &str) -> String {
    let name = crate::build_flavor::SKILL_NAME;
    let source = kit.join(name).join("SKILL.md");
    let command = format!("{} --app-data {} --endpoint {}", super::sh_quote(&data.join("bin").join(super::CLI_NAME).to_string_lossy()), super::sh_quote(&data.to_string_lossy()), super::sh_quote(endpoint));
    format!("请为你自己安装 AIMyChats 的 {name} 技能，用于通过本机 AIMyChats 读取获准的聊天记录、当前空间已有待办和已生成报告。\n\n\
1. 确认你与 AIMyChats 运行在同一台电脑。识别你实际使用的客户端或 CLI，并查明它支持的 Skills 目录；不要把其他智能体的目录当作自己的目录。若不支持本机 Skills 或目录无法确定，停止并说明原因。\n\
2. 安装源已由 AIMyChats 准备在本机：{source}。读取这个文件，将它原样安装到你确认的 Skills 目录下的 {name}/SKILL.md。先说明安装位置；若目标已有不同内容，先保留备份，不要丢弃自定义内容。无需下载、编译或覆盖全局命令。不要覆盖其他 AIMyChats 版本的技能。\n\
3. 保持 AIMyChats 打开并登录。使用下面的本机读取入口验证 health 和 policy，再查询最多 5 个可访问会话确认连接；不要读取聊天正文作为安装测试。Token 由工具自动读取，不要读取、回显或复制 Token、平台密钥等凭据。\n\n\
```sh\n{command} health\n{command} policy\n{command} sessions --limit 5\n```\n\n\
4. 如果出现 Operation not permitted、网络被禁用或访问拒绝，停止并保留原始报错。重装技能不能授予本机访问权限；不要修改全局权限、网络策略或把本机服务开放到公网。\n\
5. 分别汇报：文件是否安装、当前智能体是否识别技能、本机连接是否成功。文件安装成功不代表连接可用；需要时提示新建会话或刷新技能列表。\n", source=source.display())
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
        assert!(text.contains("sessions --limit 5"));
        assert!(text.contains("Operation not permitted"));
        assert!(!text.contains("agent-token"));
    }
}
