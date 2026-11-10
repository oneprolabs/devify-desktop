import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { defaultSettings, mergeSettings, type AppSettings } from './Settings';

type AgentInfo = {
  id: string;
  name: string;
  command: string | null;
  version: string | null;
  available: boolean;
  note: string;
};

type TestResult = { ok: boolean; text: string };

type VoiceEngine = {
  id: string;
  label: string;
  path: string | null;
  model_hint: string;
  available: boolean;
};

type VoiceStatus = {
  settings: { engine: string; model: string; language: string };
  engines: VoiceEngine[];
  selected: string | null;
};

const MAX_TIMEOUT = 900;
const MIN_TIMEOUT = 30;

export function AgentSettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [scanning, setScanning] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelError, setModelError] = useState('');
  const [testResult, setTestResult] = useState<TestResult | null>(null);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [voiceState, setVoiceState] = useState<VoiceStatus | null>(null);
  const [voiceSaving, setVoiceSaving] = useState(false);
  const [voiceNotice, setVoiceNotice] = useState('');
  const [voiceError, setVoiceError] = useState('');

  const loadVoice = useCallback(async () => {
    try {
      setVoiceState(await invoke<VoiceStatus>('wechat_voice_status'));
    } catch (e) {
      setVoiceError(String(e));
    }
  }, []);

  const scanAgents = useCallback(async () => {
    setScanning(true);
    setError('');
    try {
      setAgents(await invoke<AgentInfo[]>('agent_scan'));
    } catch (e) {
      setAgents([]);
      setError(`扫描本地 Agent 失败：${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const stored = await invoke<AppSettings>('app_settings_get');
        setSettings(mergeSettings(stored));
      } catch (e) {
        setError(String(e));
      }
      await scanAgents();
    })();
  }, [scanAgents]);

  useEffect(() => {
    void loadVoice();
  }, [loadVoice]);

  const saveVoice = async () => {
    if (!voiceState) return;
    setVoiceSaving(true);
    setVoiceNotice('');
    setVoiceError('');
    try {
      const next = await invoke<VoiceStatus>('wechat_voice_settings_save', {
        engine: voiceState.settings.engine,
        model: voiceState.settings.model,
        language: voiceState.settings.language,
      });
      setVoiceState(next);
      setVoiceNotice(next.selected ? `已保存，使用「${next.selected}」转写语音。` : '已保存，但本机还没有可用的转写引擎。');
    } catch (e) {
      setVoiceError(String(e));
    } finally {
      setVoiceSaving(false);
    }
  };

  const agent = settings.agent;
  const selected = agents.find(item => item.id === agent.provider_id) ?? null;

  const loadModels = useCallback(async (value: AppSettings['agent']) => {
    if (!value.provider_id.trim()) {
      setModels([]);
      setModelError('');
      return;
    }
    setLoadingModels(true);
    setModelError('');
    try {
      setModels(await invoke<string[]>('agent_models', { value }));
    } catch (e) {
      setModels([]);
      setModelError(String(e));
    } finally {
      setLoadingModels(false);
    }
  }, []);

  useEffect(() => {
    if (!agent.provider_id.trim()) {
      setModels([]);
      return;
    }
    void loadModels(agent);
    // Only a different agent (or executable) changes what can be listed.
  }, [agent.provider_id, agent.command, loadModels]); // eslint-disable-line react-hooks/exhaustive-deps

  const patchAgent = (patch: Partial<AppSettings['agent']>) => {
    setSettings(current => ({ ...current, agent: { ...current.agent, ...patch } }));
    setNotice('');
    setTestResult(null);
  };

  const save = async () => {
    setSaving(true);
    setError('');
    setNotice('');
    try {
      const saved = await invoke<AppSettings>('app_settings_save', { value: settings });
      setSettings(mergeSettings(saved));
      setNotice('Agent 配置已保存。聊天记录页的「AI 总结」会立即使用该配置。');
      await scanAgents();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const test = async () => {
    setTesting(true);
    setError('');
    setNotice('');
    setTestResult(null);
    try {
      const result = await invoke<{ output: string; elapsed_ms: number; command: string }>('agent_test', {
        value: agent,
      });
      const seconds = (result.elapsed_ms / 1000).toFixed(1);
      const preview = result.output.length > 160 ? `${result.output.slice(0, 160)}…` : result.output;
      setTestResult({
        ok: true,
        text: `已连接，用时 ${seconds} 秒（${result.command}）。返回：${preview}`,
      });
    } catch (e) {
      setTestResult({ ok: false, text: `测试失败：${String(e)}` });
    } finally {
      setTesting(false);
    }
  };

  const timeout = Number.isFinite(agent.timeout_seconds) ? agent.timeout_seconds : MIN_TIMEOUT;

  return (
    <div className="agent-page">
      <section className="panel">
        <h2>本地总结 Agent</h2>
        <p className="help">
          扫描本机已安装的命令行 Agent，选择一个用于聊天记录总结。应用只负责把整理好的原文交给它，
          登录状态、模型和额度仍由该 Agent 自己管理。
        </p>
        <div className="config warn">
          聊天内容会通过所选 Agent 及其模型服务处理，可能离开本机。只在确认原文范围后再生成。
        </div>

        <div className="settings-subheading">
          <strong>本地检测结果</strong>
          <span>{scanning ? '正在扫描…' : `检测到 ${agents.filter(item => item.available).length} 个可用 Agent`}</span>
        </div>
        <div className="agent-picker">
          <fieldset className="agent-options">
            <legend>选择本地 Agent</legend>
            {agents.map(item => <label key={item.id} className={`agent-option${agent.provider_id === item.id ? ' selected' : ''}${item.available ? '' : ' unavailable'}`}>
              <input type="radio" name="local-agent" value={item.id} checked={agent.provider_id === item.id} disabled={scanning || !item.available} onChange={() => patchAgent({ provider_id: item.id })}/>
              <span><strong>{item.name}</strong><small>{item.available ? '已检测到' : '未检测到'}{item.version ? ` · ${item.version}` : ''}</small></span>
            </label>)}
          </fieldset>
          {selected && (
            <div className="agent-detail">
              <span className={`agent-state ${selected.available ? 'ok' : 'warn'}`}>
                {selected.available ? '可用' : '未检测到'}
              </span>
              <span className="agent-detail-path mono">{selected.command ?? '—'}</span>
              <span className="agent-detail-note">{selected.version ? `${selected.version} · ${selected.note}` : selected.note}</span>
            </div>
          )}
          {!agents.length && !scanning && <p className="help">没有检测到受支持的 Agent。</p>}
          <div className="agent-actions">
            <button type="button" className="secondary" onClick={() => void scanAgents()} disabled={scanning}>
              {scanning ? '扫描中…' : '重新扫描'}
            </button>
            <button type="button" className="primary" onClick={() => void test()} disabled={testing || !agent.provider_id}>
              {testing ? '测试中…' : '测试连接'}
            </button>
          </div>
        </div>
        {testResult && <div className={`config ${testResult.ok ? 'ok' : 'warn'}`}>{testResult.text}</div>}
        {agent.provider_id && !selected?.available && (
          <p className="help">
            已保存的「{agent.provider_id}」当前不可用，请重新选择，或在下方填写可执行文件路径。
          </p>
        )}
      </section>

      <section className="panel">
        <h2>生成参数</h2>
        <div className="grid">
          <label>可执行文件路径（可选）
            <input
              value={agent.command}
              onChange={event => patchAgent({ command: event.target.value })}
              placeholder="留空则自动在 PATH 中查找"
              spellCheck={false}
            />
          </label>
          <label>模型（可选）
            <input
              list="agent-model-options"
              value={agent.model}
              onChange={event => patchAgent({ model: event.target.value })}
              placeholder={models.length ? '从建议中选择，或直接填写 provider/model' : '留空使用该 Agent 的默认模型'}
              spellCheck={false}
            />
            <datalist id="agent-model-options">
              {models.map(model => <option key={model} value={model} />)}
            </datalist>
          </label>
          <label>超时（秒）
            <input
              type="number"
              min={MIN_TIMEOUT}
              max={MAX_TIMEOUT}
              value={timeout}
              onChange={event => patchAgent({ timeout_seconds: Number(event.target.value) || MIN_TIMEOUT })}
            />
          </label>
        </div>
        <div className="agent-model-hint">
          <span>
            {loadingModels
              ? '正在读取该 Agent 的模型列表…'
              : modelError
                ? modelError
                : models.length
                  ? `已读取 ${models.length} 个模型，可在输入框中直接选择。`
                  : agent.provider_id
                    ? '该 Agent 不支持列出模型，请按它自己的配置填写。'
                    : '先选择一个 Agent，再填写模型。'}
          </span>
          <button type="button" className="secondary" onClick={() => void loadModels(agent)} disabled={!agent.provider_id.trim() || loadingModels}>
            刷新模型
          </button>
        </div>
        <label className="agent-instructions">总结补充要求（可选）
          <textarea
            rows={3}
            maxLength={2000}
            value={agent.instructions}
            onChange={event => patchAgent({ instructions: event.target.value })}
            placeholder="例如：重点关注交付风险和负责人；忽略闲聊内容。"
          />
        </label>
        <p className="help">
          不填模型时使用该 Agent 的默认模型；如果默认模型的凭据已失效（例如 OpenCode 的默认模型指向未登录的服务），
          在这里选一个它已配置的模型即可。补充要求会追加到内置总结提示词，不能覆盖“只依据原文、不编造”的约束。超时时间限制在
          {MIN_TIMEOUT}–{MAX_TIMEOUT} 秒；超时或失败不会写入任何总结结果。
        </p>
      </section>

      <section className="panel">
        <h2>语音转写</h2>
        <p className="help">
          微信语音是 SILK v3，应用内置解码器把它转成 24 kHz WAV，再交给本机的转写命令行工具。
          转写在本机执行，结果按账号和消息缓存，不会重复计算；开启上传后只提交转写文本，不上传原始语音。
        </p>
        <label className="confirm">
          <input
            type="checkbox"
            checked={settings.voice.auto_transcribe}
            onChange={event => setSettings(current => ({
              ...current,
              voice: { ...current.voice, auto_transcribe: event.target.checked },
            }))}
          />
          <span>上传前自动转写新的语音</span>
        </label>
        <label className="confirm">
          <input
            type="checkbox"
            checked={settings.voice.upload_transcripts}
            onChange={event => setSettings(current => ({
              ...current,
              voice: { ...current.voice, upload_transcripts: event.target.checked },
            }))}
          />
          <span>将语音转写文本上传到 Devify</span>
        </label>
        <p className="help">默认只上传转写文本，不上传原始语音文件。没有本地引擎或转写失败时，会保留待转写状态并在下次同步重试。</p>
        {voiceState ? (
          <>
            <div className="settings-subheading">
              <strong>本机检测结果</strong>
              <span>
                {voiceState.engines.filter(engine => engine.available).length
                  ? `可用：${voiceState.engines.filter(engine => engine.available).map(engine => engine.label).join('、')}`
                  : '没有检测到转写引擎'}
              </span>
            </div>
            <div className="grid">
              <label>转写引擎
                <select
                  value={voiceState.settings.engine}
                  onChange={event =>
                    setVoiceState(current =>
                      current ? { ...current, settings: { ...current.settings, engine: event.target.value } } : current,
                    )
                  }
                >
                  <option value="">自动选择（第一个可用）</option>
                  {voiceState.engines.map(engine => (
                    <option key={engine.id} value={engine.id} disabled={!engine.available}>
                      {engine.label} · {engine.available ? '可用' : '未检测到'}
                    </option>
                  ))}
                </select>
              </label>
              <label>模型 / 模型路径
                <input
                  value={voiceState.settings.model}
                  onChange={event =>
                    setVoiceState(current =>
                      current ? { ...current, settings: { ...current.settings, model: event.target.value } } : current,
                    )
                  }
                  placeholder="留空使用引擎默认值"
                  spellCheck={false}
                />
              </label>
              <label>语言
                <input
                  value={voiceState.settings.language}
                  onChange={event =>
                    setVoiceState(current =>
                      current ? { ...current, settings: { ...current.settings, language: event.target.value } } : current,
                    )
                  }
                  placeholder="zh"
                  spellCheck={false}
                />
              </label>
            </div>
            <p className="help">
              {voiceState.engines.find(engine => engine.id === (voiceState.settings.engine || voiceState.selected))
                ?.model_hint ?? '选择一个引擎后，这里会显示它的模型说明。'}
            </p>
            <div className="agent-actions">
              <button type="button" className="secondary" onClick={() => void loadVoice()}>
                重新检测
              </button>
              <button type="button" className="primary" onClick={() => void saveVoice()} disabled={voiceSaving}>
                {voiceSaving ? '正在保存…' : '保存语音设置'}
              </button>
            </div>
          </>
        ) : (
          <p className="help">{voiceError || '正在检测本机转写引擎…'}</p>
        )}
        {voiceNotice && <div className="config ok">{voiceNotice}</div>}
        {voiceError && voiceState && <div className="config warn">{voiceError}</div>}
      </section>

      {notice && <div className="notice">{notice}</div>}
      {error && <div className="error">{error}</div>}
      <div className="page-actions">
        <button className="primary" disabled={saving} onClick={() => void save()}>
          {saving ? '正在保存…' : '保存 Agent 配置'}
        </button>
      </div>
    </div>
  );
}
