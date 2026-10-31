import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type AccessTarget = {
  id: string;
  name: string;
  available: boolean;
  command: string | null;
  skills_dir: string;
  installed: string[];
};

type SkillAsset = {
  name: string;
  description: string;
  download_url: string;
};

type WxqAsset = {
  path: string;
  size: number;
  download_url: string;
};

type AccessStatus = {
  running: boolean;
  base_url: string;
  token_required: boolean;
  wxq: WxqAsset | null;
  skills: SkillAsset[];
  targets: AccessTarget[];
  default_target: string;
  default_bin_dir: string;
  default_skills_dir: string;
  manifest_url: string;
  install_url: string;
};

type PromptOut = {
  prompt: string;
  bin_dir: string;
  skills_dir: string;
};

function sizeLabel(bytes: number): string {
  if (!bytes) return '';
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function AgentAccess() {
  const [status, setStatus] = useState<AccessStatus | null>(null);
  const [tool, setTool] = useState('');
  const [binDir, setBinDir] = useState('');
  const [skillsDir, setSkillsDir] = useState('');
  const [prompt, setPrompt] = useState('');
  const [expanded, setExpanded] = useState({ binDir: '', skillsDir: '' });
  const [loadingPrompt, setLoadingPrompt] = useState(false);
  const [building, setBuilding] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const initialized = useRef(false);
  const skillsTouched = useRef(false);

  const load = useCallback(async (forceDefaults = false) => {
    try {
      const next = await invoke<AccessStatus>('agent_access_status');
      setStatus(next);
      setError('');
      if (!initialized.current || forceDefaults) {
        initialized.current = true;
        skillsTouched.current = false;
        setTool(next.default_target);
        setBinDir(next.default_bin_dir);
        setSkillsDir(next.default_skills_dir);
      }
    } catch (e) {
      setError(`读取接入状态失败：${String(e)}`);
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(timer);
  }, [load]);

  const selected = status?.targets.find(item => item.id === tool) ?? null;

  const pickTool = (id: string) => {
    setTool(id);
    setNotice('');
    const next = status?.targets.find(item => item.id === id);
    if (next && !skillsTouched.current) {
      setSkillsDir(next.skills_dir);
    }
  };

  useEffect(() => {
    if (!status || !tool || !binDir.trim() || !skillsDir.trim()) {
      setPrompt('');
      return;
    }
    const timer = window.setTimeout(() => {
      setLoadingPrompt(true);
      invoke<PromptOut>('agent_access_prompt', { tool, binDir, skillsDir })
        .then(out => {
          setPrompt(out.prompt);
          setExpanded({ binDir: out.bin_dir, skillsDir: out.skills_dir });
          setError('');
        })
        .catch(e => {
          setPrompt('');
          setExpanded({ binDir: '', skillsDir: '' });
          setError(String(e));
        })
        .finally(() => setLoadingPrompt(false));
    }, 220);
    return () => window.clearTimeout(timer);
  }, [status?.running, status?.base_url, tool, binDir, skillsDir]);

  const copyPrompt = async () => {
    setNotice('');
    try {
      await navigator.clipboard.writeText(prompt);
      setNotice(`提示词已复制。粘贴到 ${selected?.name ?? '你的 Agent'} 后，允许它执行 shell 并写入上面两个目录。`);
    } catch (e) {
      setError(`复制失败，请手动选择提示词文本：${String(e)}`);
    }
  };

  const buildWxq = async () => {
    setBuilding(true);
    setNotice('');
    setError('');
    try {
      const path = await invoke<string>('agent_build_wxq');
      setNotice(`已构建可下载的 wxq：${path}`);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBuilding(false);
    }
  };

  const stateLabel = status === null ? '正在读取状态…' : status.running ? '服务运行中' : '服务未运行';
  const installedCount = selected?.installed.length ?? 0;
  const base = status?.base_url ?? 'http://127.0.0.1:18765';

  return <div className="access-page">
    <section className="panel">
      <div className="mcp-head">
        <div>
          <h2>工具接入</h2>
          <p className="help">
            复制一条提示词，粘贴到你正在使用的 Agent。它会从本机服务下载只读 CLI <code>wxq</code> 和三个
            <code>SKILL.md</code>，写入你指定的目录，并用 <code>wxq health</code> / <code>wxq policy</code> 验证连接。
            服务只在本机运行，应用关闭后停止。
          </p>
        </div>
        <span className={`mcp-state ${status?.running ? 'on' : 'off'}`}>
          <span className="mcp-state-dot" />{stateLabel}
        </span>
      </div>

      <dl className="mcp-meta">
        <div>
          <dt>本机服务</dt>
          <dd><code>{base}</code></dd>
        </div>
        <div>
          <dt>CLI 下载</dt>
          <dd><code>{base}/agent/wxq</code>{status?.wxq ? ` · ${sizeLabel(status.wxq.size)}` : ' · 不可用'}</dd>
        </div>
        <div>
          <dt>Skills 清单</dt>
          <dd><code>{base}/agent/skills</code></dd>
        </div>
        <div>
          <dt>认证</dt>
          <dd>{status?.token_required ? '需要 Bearer Token' : '仅本机访问'}</dd>
        </div>
      </dl>

      {status && !status.running && (
        <div className="config warn">
          服务没有监听，提示词里的下载会失败。请确认 AImyChats 仍在运行；如果端口被占用，可在启动前设置
          <code>WECHAT_AGENT_BIND</code> 换一个地址。
        </div>
      )}
      {status && !status.wxq && (
        <div className="config warn">
          <div>当前没有可下载的 wxq。在源码检出版本里可以点下面的按钮构建（release），构建完成后状态会自动刷新；打包版需要另行内置该二进制。</div>
          <div className="mcp-actions">
            <button className="secondary" onClick={() => void buildWxq()} disabled={building}>
              {building ? '正在构建 wxq…' : '构建 wxq'}
            </button>
          </div>
        </div>
      )}
    </section>

    <section className="panel">
      <h2>安装位置</h2>
      <div className="grid access-paths">
        <label>当前工具
          <select value={tool} onChange={event => pickTool(event.target.value)} disabled={!status}>
            {(status?.targets ?? []).map(item => <option key={item.id} value={item.id}>
              {item.name}{item.available ? '' : ' · 未检测到'}
            </option>)}
          </select>
        </label>
        <label>CLI 目录
          <input
            value={binDir}
            onChange={event => { setBinDir(event.target.value); setNotice(''); }}
            placeholder="~/.local/bin"
            spellCheck={false}
          />
        </label>
        <label>Skills 目录
          <input
            value={skillsDir}
            onChange={event => { setSkillsDir(event.target.value); skillsTouched.current = true; setNotice(''); }}
            placeholder="~/.claude/skills"
            spellCheck={false}
          />
        </label>
      </div>
      <p className="help">
        CLI 会写成 <code>{expanded.binDir || binDir || '—'}/wxq</code>；Skills 会写成
        <code>{expanded.skillsDir || skillsDir || '—'}/&lt;skill&gt;/SKILL.md</code>。
        支持 <code>~</code> 和绝对路径；选择工具只会帮你填入它的默认 Skills 目录，仍然可以手动改。
      </p>
      {selected && (
        <div className="agent-detail">
          <span className={`agent-state ${selected.available ? 'ok' : 'warn'}`}>
            {selected.available ? '已检测到' : '未检测到'}
          </span>
          <span className="agent-detail-path mono">{selected.command ?? selected.skills_dir}</span>
          <span className="agent-detail-note">
            {installedCount === 3
              ? '三个 Skill 已安装'
              : installedCount > 0
                ? `已安装 ${installedCount}/3 个 Skill`
                : '尚未安装 Skill'}
          </span>
        </div>
      )}
    </section>

    <section className="panel">
      <h2>接入提示词</h2>
      <p className="help">
        在 {selected?.name ?? '目标 Agent'} 中新建一个会话，粘贴下面全部内容并允许它执行 shell。
        提示词只包含下载、写入和验证步骤，不包含任何聊天内容。
      </p>
      <pre className="mcp-code access-prompt">{loadingPrompt && !prompt ? '正在生成提示词…' : prompt || '填写上面的两个目录后会自动生成提示词。'}</pre>
      <div className="mcp-actions">
        <button className="primary access-copy" onClick={() => void copyPrompt()} disabled={!prompt || loadingPrompt}>
          复制提示词
        </button>
        <button className="secondary" onClick={() => void load(true)}>恢复默认目录</button>
      </div>
      {notice && <div className="notice">{notice}</div>}
      {error && <div className="error">{error}</div>}
    </section>

    <section className="panel">
      <h2>可以下载什么</h2>
      <ul className="mcp-tools">
        <li>
          <code>wxq</code>
          <span>只读命令行：health、policy、sessions、messages、search、digest。不读数据库、不解密、不发邮件。</span>
        </li>
        {status?.skills.map(skill => <li key={skill.name}>
          <code>{skill.name}</code>
          <span>{skill.description}</span>
        </li>)}
      </ul>
      <p className="help">
        下载地址：CLI <code>{base}/agent/wxq</code>；清单 <code>{status?.manifest_url ?? `${base}/agent/manifest`}</code>；
        安装脚本 <code>{status?.install_url ?? `${base}/agent/install.sh`}</code>（可加
        <code>?bin=&lt;目录&gt;&amp;skills=&lt;目录&gt;</code>）。这些接口和聊天读取接口使用同一套白名单与 Token 规则。
      </p>
      <ul className="mcp-checklist">
        <li>安装后运行 <code>{expanded.binDir || binDir || '~/.local/bin'}/wxq health</code>，应返回 <code>ok · wechat-devify-bridge</code>。</li>
        <li><code>wxq policy</code> 应显示当前账号和白名单会话；白名单为空时不会返回任何会话。</li>
        <li>CLI 与 Skills 都写入你指定的目录，不影响微信原始数据库，也不会自动发送邮件。</li>
      </ul>
    </section>
  </div>;
}
