import { t, tf, ctf } from '../../locale';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { defaultSettings, mergeSettings, mergeSavedSettings, requireSettingsRevision, settingsSaveError, type AppSettings } from '../settings/Settings';

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

export function AgentSettings({ summaryOnly = false }: { summaryOnly?: boolean } = {}) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [loaded, setLoaded] = useState(false);
  const [advanced,setAdvanced] = useState(false);
  const testLock=useRef(false);
  const saveLock = useRef(false);
  const confirmedRevision = useRef<number | null>(null);
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
  const [saveError, setSaveError] = useState('');
  const [voiceState, setVoiceState] = useState<VoiceStatus | null>(null);
  const [voiceOpen,setVoiceOpen] = useState(false);
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
    try {
      setAgents(await invoke<AgentInfo[]>('agent_scan'));
    } catch (e) {
      setAgents([]);
      setError(tf("扫描本地 Agent 失败：{0}", String(e)));
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const stored = await invoke<AppSettings>('app_settings_get');
        const next = mergeSettings(requireSettingsRevision(stored));
        confirmedRevision.current = next.user_settings_revision;
        setSettings(next);
        setLoaded(true);
      } catch (e) {
        setError(String(e));
      }
      await scanAgents();
    })();
  }, [scanAgents]);

  useEffect(() => {
    if(voiceOpen)void loadVoice();
  }, [loadVoice,voiceOpen]);

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
      setVoiceNotice(next.selected ? tf("已保存，使用「{0}」转写语音。", next.selected) : t("已保存，但本机还没有可用的转写引擎。"));
    } catch (e) {
      setVoiceError(String(e));
    } finally {
      setVoiceSaving(false);
    }
  };

  const agent = settings.agent;
  const automatic = agents.find(item=>item.id==='codex'&&item.available) ?? agents.find(item=>item.available) ?? null;
  const selected = agent.provider_id ? agents.find(item => item.id === agent.provider_id) ?? null : automatic;

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
    if (!advanced || !agent.provider_id.trim()) {
      setModels([]);
      return;
    }
    void loadModels(agent);
    // Only a different agent (or executable) changes what can be listed.
  }, [agent.provider_id, agent.command, loadModels, advanced]); // eslint-disable-line react-hooks/exhaustive-deps

  const patchAgent = (patch: Partial<AppSettings['agent']>) => {
    setSettings(current => ({ ...current, agent: { ...current.agent, ...patch } }));
    setNotice('');
    setTestResult(null);
  };

  const save = async () => {
    if (!loaded || confirmedRevision.current === null || saveLock.current) return;
    saveLock.current = true;
    setSaving(true);
    setSaveError('');
    setError('');
    setNotice('');
    try {
      const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: { ...settings, user_settings_revision: confirmedRevision.current } })));
      confirmedRevision.current = saved.user_settings_revision;
      setSettings(current => mergeSavedSettings(saved, settings, current));
      setNotice(t(summaryOnly ? '本地总结设置已保存。' : "Agent 配置已保存。聊天记录页的「AI 总结」会立即使用该配置。"));
      await scanAgents();
    } catch (e) {
      setSaveError(settingsSaveError(e));
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };

  const test = async () => {
    if(testLock.current)return;testLock.current=true;
    setTesting(true);
    setError('');
    setNotice('');
    setTestResult(null);
    try {
      const result = await invoke<{ output: string; elapsed_ms: number; command: string }>('agent_test', {
        value: agent.provider_id ? agent : {...agent,provider_id:automatic?.id??''},
      });
      const seconds = (result.elapsed_ms / 1000).toFixed(1);
      const preview = result.output.length > 160 ? `${result.output.slice(0, 160)}…` : result.output;
      setTestResult({
        ok: true,
        text: tf("已连接，用时 {0} 秒（{1}）。返回：{2}", seconds, result.command, preview),
      });
    } catch (e) {
      setTestResult({ ok: false, text: tf("测试失败：{0}", String(e)) });
    } finally {
      testLock.current=false;
      setTesting(false);
    }
  };

  const timeout = Number.isFinite(agent.timeout_seconds) ? agent.timeout_seconds : MIN_TIMEOUT;

  if (!loaded) return <div className="agent-page">{error ? <div className="error">{t("设置加载失败：")}{t(error)} {t("请重新打开页面。")}</div> : <p>{t("正在加载设置…")}</p>}</div>;

  return (
    <div className="agent-page">
      <section className="panel">
        {!summaryOnly && <h2>{t("本地工具总结")}</h2>}
        <p className="help">
          {t("自动识别本机工具，沿用已有的登录状态和默认模型，无需填写密钥。优先使用 Codex，也可以手动选择。")}
        </p>
        <div className="config warn">
          {t("聊天内容会通过所选 Agent 及其模型服务处理，可能离开本机。只在确认原文范围后再生成。")}
        </div>

        <div className="settings-subheading">
          <strong>{t("本地检测结果")}</strong>
          <span>{scanning ? t("正在扫描…") : tf("检测到 {0} 个可用 Agent", agents.filter(item => item.available).length)}</span>
        </div>
        <div className="agent-picker">
          <fieldset className="agent-options">
            <legend>{t("选择本地 Agent")}</legend>
            <label className={`agent-option${!agent.provider_id?' selected':''}`}><input type="radio" name="local-agent" checked={!agent.provider_id} disabled={scanning} onChange={()=>patchAgent({provider_id:'',command:'',model:''})}/><span><strong>{t('自动选择（推荐）')}</strong><small>{automatic?tf('当前可用：{0}',automatic.name):t('安装并登录 Codex 后，点击重新检测。')}</small></span></label>
            {agents.filter(item=>item.available||item.id===agent.provider_id).map(item => <label key={item.id} className={`agent-option${agent.provider_id === item.id ? ' selected' : ''}${item.available ? '' : ' unavailable'}`}>
              <input type="radio" name="local-agent" value={item.id} checked={agent.provider_id === item.id} disabled={scanning || !item.available} onChange={() => patchAgent({ provider_id: item.id })}/>
              <span><strong>{item.name}</strong><small>{item.available ? t("已检测到") : t("未检测到")}{item.version ? ` · ${item.version}` : ''}</small></span>
            </label>)}
          </fieldset>
          {selected && (
            <div className="agent-detail">
              <span className={`agent-state ${selected.available ? 'ok' : 'warn'}`}>
                {selected.available ? t("可用") : t("未检测到")}
              </span>
              <span>{selected.name}{selected.version?` · ${selected.version}`:''}</span>
            </div>
          )}
          {!agents.length && !scanning && <p className="help">{t("没有检测到受支持的 Agent。")}</p>}
          <div className="agent-actions">
            <button type="button" className="secondary" onClick={() => void scanAgents()} disabled={scanning}>
              {scanning ? t("扫描中…") : t("重新扫描")}
            </button>
            <button type="button" className="primary" onClick={() => void test()} disabled={testing || !selected?.available}>
              {testing ? t("测试中…") : t("测试连接")}
            </button>
          </div>
        </div>
        {testResult && <div className={`config ${testResult.ok ? 'ok' : 'warn'}`}>{testResult.text}</div>}
        {agent.provider_id && !selected?.available && (
          <p className="help">
            {t("已保存的「")}{agent.provider_id}{t("」当前不可用，请重新选择，或在下方填写可执行文件路径。")}
          </p>
        )}
      </section>

      <details className="panel agent-advanced" open={advanced} onToggle={event=>setAdvanced(event.currentTarget.open)}>
        <summary>{t("高级设置")}<span>{t('通常无需修改')}</span></summary>
        <div className="grid">
          <label>{t("可执行文件路径（可选）")}
            <input
              value={agent.command}
              onChange={event => patchAgent({ command: event.target.value })}
              placeholder={t("留空则自动在 PATH 中查找")}
              spellCheck={false}
            />
          </label>
          <label>{t("模型（可选）")}
            <input
              list="agent-model-options"
              value={agent.model}
              onChange={event => patchAgent({ model: event.target.value })}
              placeholder={models.length ? t("从建议中选择，或直接填写 provider/model") : t("留空使用该 Agent 的默认模型")}
              spellCheck={false}
            />
            <datalist id="agent-model-options">
              {models.map(model => <option key={model} value={model} />)}
            </datalist>
          </label>
          <label>{t("超时（秒）")}
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
              ? t("正在读取该 Agent 的模型列表…")
              : modelError
                ? modelError
                : models.length
                  ? tf("已读取 {0} 个模型，可在输入框中直接选择。", models.length)
                  : agent.provider_id
                    ? t("该 Agent 不支持列出模型，请按它自己的配置填写。")
                    : t("先选择一个 Agent，再填写模型。")}
          </span>
          <button type="button" className="secondary" onClick={() => void loadModels(agent)} disabled={!agent.provider_id.trim() || loadingModels}>
            {t("刷新模型")}
          </button>
        </div>
        <label className="agent-instructions">{t("总结补充要求（可选）")}
          <textarea
            rows={3}
            maxLength={2000}
            value={agent.instructions}
            onChange={event => patchAgent({ instructions: event.target.value })}
            placeholder={t("例如：重点关注交付风险和负责人；忽略闲聊内容。")}
          />
        </label>
        <p className="help">
          {t("不填模型时使用该 Agent 的默认模型；如果默认模型的凭据已失效（例如 OpenCode 的默认模型指向未登录的服务），\n          在这里选一个它已配置的模型即可。补充要求会追加到内置总结提示词，不能覆盖“只依据原文、不编造”的约束。超时时间限制在")}
          {MIN_TIMEOUT}–{MAX_TIMEOUT} {t("秒；超时或失败不会写入任何总结结果。")}
        </p>
      </details>

      {!summaryOnly && <details className="panel agent-advanced" onToggle={event=>setVoiceOpen(event.currentTarget.open)}>
        <summary>{t("语音转写")}<span>{t('可选')}</span></summary>
        <p className="help">
          {t("微信语音是 SILK v3，应用内置解码器把它转成 24 kHz WAV，再交给本机的转写命令行工具。\n          转写在本机执行，结果按账号和消息缓存，不会重复计算；开启上传后只提交转写文本，不上传原始语音。")}
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
          <span>{t("上传前自动转写新的语音")}</span>
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
          <span>{t("将语音转写文本上传到服务端")}</span>
        </label>
        <p className="help">{t("默认只上传转写文本，不上传原始语音文件。没有本地引擎或转写失败时，会保留待转写状态并在下次同步重试。")}</p>
        {voiceState ? (
          <>
            <div className="settings-subheading">
              <strong>{t("本机检测结果")}</strong>
              <span>
                {voiceState.engines.filter(engine => engine.available).length
                  ? tf("可用：{0}", voiceState.engines.filter(engine => engine.available).map(engine => engine.label).join('、'))
                  : t("没有检测到转写引擎")}
              </span>
            </div>
            <div className="grid">
              <label>{t("转写引擎")}
                <select
                  value={voiceState.settings.engine}
                  onChange={event =>
                    setVoiceState(current =>
                      current ? { ...current, settings: { ...current.settings, engine: event.target.value } } : current,
                    )
                  }
                >
                  <option value="">{t("自动选择（第一个可用）")}</option>
                  {voiceState.engines.map(engine => (
                    <option key={engine.id} value={engine.id} disabled={!engine.available}>
                      {t(engine.label)} · {engine.available ? t("可用") : t("未检测到")}
                    </option>
                  ))}
                </select>
              </label>
              <label>{t("模型 / 模型路径")}
                <input
                  value={voiceState.settings.model}
                  onChange={event =>
                    setVoiceState(current =>
                      current ? { ...current, settings: { ...current.settings, model: event.target.value } } : current,
                    )
                  }
                  placeholder={t("留空使用引擎默认值")}
                  spellCheck={false}
                />
              </label>
              <label>{t("语言")}
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
              {t(voiceState.engines.find(engine => engine.id === (voiceState.settings.engine || voiceState.selected))
                ?.model_hint || "选择一个引擎后，这里会显示它的模型说明。")}
            </p>
            <div className="agent-actions">
              <button type="button" className="secondary" onClick={() => void loadVoice()}>
                {t("重新检测")}
              </button>
              <button type="button" className="primary" onClick={() => void saveVoice()} disabled={voiceSaving}>
                {voiceSaving ? t("正在保存…") : t("保存语音设置")}
              </button>
            </div>
          </>
        ) : (
          <p className="help">{voiceError || t("正在检测本机转写引擎…")}</p>
        )}
        {voiceNotice && <div className="config ok">{voiceNotice}</div>}
        {voiceError && voiceState && <div className="config warn">{voiceError}</div>}
      </details>}

      {notice && <div className="notice">{notice}</div>}
      {error && <div className="error">{t(error)}</div>}
      {saveError && <div className="error" role="alert">{saveError}</div>}
      <div className="page-actions">
        <button className="primary" disabled={saving} onClick={() => void save()}>
          {saving ? t("正在保存…") : t(summaryOnly ? '保存本地总结设置' : "保存 Agent 配置")}
        </button>
      </div>
    </div>
  );
}
