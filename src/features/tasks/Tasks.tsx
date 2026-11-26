import { useEffect, useRef, useState } from 'react';
import { dateLocale, getUiLanguage, t, tf } from '../../shared/i18n/locale';
import { errorText } from '../../shared/i18n/errorText';
import { desktopInvoke as invoke } from '../../shared/api/desktopInvoke';
import { useTaskRuntime, refreshTaskRuntime, type RuntimeJob } from './taskRuntime';
import { WizardActionButton } from '../../shared/ui/WizardActionButton';
import { BackupRuntimeSwitch } from '../backup/BackupRuntimeSwitch';

const emptyBackup = (job: { kind?: string; status: string; message: string }) =>
  job.kind !== 'summary' && job.status === 'completed'
  && (job.message === '本批未新增消息' || /^服务端解析完成，共 0 条消息$/.test(job.message));
const labels: Record<string, string> = { queued: '等待备份', running: '正在准备', uploading: '正在上传', processing: '服务端处理中', completed: '已完成', failed: '等待重试', cancelled: '已取消' };
function taskMessage(job: RuntimeJob): string {
  if (emptyBackup(job)) return t('本批未新增消息');
  if (job.summary_progress) {
    const { completed, total } = job.summary_progress;
    const progress = tf('已完成 {0} / {1} 批。', completed, total);
    if (job.status === 'running') return completed === total ? t('正在合并分批摘要。') : progress;
    if (job.status === 'failed') return `${progress} ${errorText(job.message)}`;
  }
  const parsed = /^服务端解析完成，共 (\d+) 条消息$/.exec(job.message);
  if (parsed) return tf('服务端解析完成，共 {0} 条消息', Number(parsed[1]));
  return errorText(job.message, job.status === 'failed' ? undefined : labels[job.status]);
}

export function Tasks({ onContinueSetup, onRecoverWechatKeys }: { onNewUpload: () => void; startCreating?: boolean; onContinueSetup?: (platform: string) => void; onRecoverWechatKeys: (accountId: string) => void }) {
  const runtime = useTaskRuntime();
  const [error, setError] = useState('');
  const [page, setPage] = useState(0);
  const [cloud, setCloud] = useState<Array<{ id: number; source_type: string; status: string; message_count: number; error_message?: string }>>([]);
  const [cloudPage, setCloudPage] = useState(0);
  const [cloudMore, setCloudMore] = useState(false);
  const [cloudBusy, setCloudBusy] = useState(false);
  const [nameSnapshot, setNameSnapshot] = useState<{ context?: string | null; names: Record<string, string> }>({ names: {} });
  const context = useRef(runtime.context);
  context.current = runtime.context;
  const request = useRef(0);
  useEffect(() => {
    request.current += 1;
    setPage(0); setCloud([]); setCloudPage(0); setCloudMore(false); setCloudBusy(false); setError('');
    return () => { request.current += 1; };
  }, [runtime.context]);
  const loadCloud = async (target = 1) => {
    if (cloudBusy) return;
    setCloudBusy(true); setError('');
    const id = ++request.current; const owner = context.current;
    const current = () => id === request.current && owner === context.current;
    try {
      const settings = await invoke<{ devify: { active_space_id?: string } }>('app_settings_get');
      if (!current()) return;
      if (!settings.devify.active_space_id) throw new Error('请先选择空间。');
      const data = await invoke<{ list: typeof cloud; pagination?: { next?: unknown } }>('devify_upload_list', { spaceId: settings.devify.active_space_id, page: target, pageSize: 20 });
      if (!current()) return;
      setCloud(data.list ?? []); setCloudPage(target); setCloudMore(Boolean(data.pagination?.next));
    } catch (reason) { if (current()) setError(String(reason)); }
    finally { if (current()) setCloudBusy(false); }
  };
  const emptyCounts = new Map<string, number>();
  const grouped = new Map<string, string>();
  const jobs = [...runtime.jobs].sort((a,b) => b.updated_at.localeCompare(a.updated_at)).filter(job => {
    if (!emptyBackup(job)) return true;
    const key = JSON.stringify([job.account_id, [...(job.scope?.usernames ?? [])].sort(), new Date(job.updated_at).toDateString()]);
    const first = grouped.get(key);
    if (first !== undefined) {
      emptyCounts.set(first, (emptyCounts.get(first) ?? 1) + 1);
      return false;
    }
    grouped.set(key, job.id);
    return true;
  });
  const activePage = Math.min(page, Math.max(0, Math.ceil(jobs.length / 20) - 1));
  const visible = jobs.slice(activePage * 20, (activePage + 1) * 20);
  // At most two names per visible task; progress events must not rescan metadata.
  const nameTargets = JSON.stringify(visible.map(job => [job.account_id, job.scope?.usernames.slice(0, 2) ?? []]));
  useEffect(() => {
    let cancelled = false;
    const owner = runtime.context;
    const groups = new Map<string, Set<string>>();
    for (const [account, names] of JSON.parse(nameTargets) as [string, string[]][]) {
      const wanted = groups.get(account) ?? new Set<string>();
      names.forEach(name => wanted.add(name));
      if (wanted.size) groups.set(account, wanted);
    }
    void (async () => {
      const names: Record<string, string> = {};
      // Sequential source reads stay within the shared local read budget.
      for (const [account, wanted] of groups) {
        if (cancelled) return;
        try {
          const rows = await invoke<Array<{ username: string; display: string }>>('chat_sessions', { accountId: account, focusUsernames: [...wanted], limit: wanted.size });
          for (const row of rows) if (row.display?.trim() && row.display !== row.username) names[JSON.stringify([account, row.username])] = row.display.trim();
        } catch { /* Names are optional; keep the task count and status usable. */ }
      }
      if (!cancelled && owner === context.current) setNameSnapshot({ context: owner, names });
    })();
    return () => { cancelled = true; };
  }, [runtime.context, nameTargets]);
  const taskTitle = (job: typeof jobs[number]) => {
    if(job.kind==='summary')return tf('聊天总结 · {0}',job.title??t('聊天'));
    const source = job.account_id.endsWith('@s.whatsapp.net') ? 'WhatsApp' : t('微信');
    const total = job.scope?.usernames.length ?? 0;
    const names = nameSnapshot.context === runtime.context ? (job.scope?.usernames.slice(0, 2) ?? []).map(name => nameSnapshot.names[JSON.stringify([job.account_id, name])]).filter(Boolean) : [];
    const separator = getUiLanguage() === 'English' ? ', ' : '、';
    const summary = names.length ? (names.length === total ? names.join(separator) : tf('{0}等 {1} 个会话', names.join(separator), total)) : tf('{0} 个会话', total);
    return tf('备份{0} · {1}', source, summary);
  };
  return <section className="settings-block task-runtime-page">
    <header className="task-runtime-header"><div><h2>{t('任务')}</h2><p>{runtime.paused ? t('自动备份已暂停，手动创建的总结任务仍会继续。') : runtime.running ? t('正在后台处理任务。') : t('备份与聊天总结的执行记录会显示在这里。')}</p></div>
      <BackupRuntimeSwitch showLabel onError={setError}/>
    </header>
    {runtime.pending_config > 0 && <p role="status">{t('设置已保存在本地，正在等待同步。同步完成后自动继续备份。')}</p>}
    {[...new Set((runtime.sources ?? []).filter(source => !source.setup_complete).map(source => source.platform))].map(platform => <div key={platform} className="task-setup-notice" role="status"><p>{tf('{0} 配置未完成，备份已暂停。', platform === 'whatsapp' ? 'WhatsApp' : t('微信'))}</p>{onContinueSetup && <button type="button" className="wxv-btn" onClick={() => onContinueSetup(platform)}>{t('继续配置')}</button>}</div>)}
    {runtime.sources?.filter(source => source.backup_needs_keys && source.setup_complete && source.backup_enabled).map(source => (
      <div key={source.account_id} className="task-setup-notice">
        <p role="alert">{t('微信备份已暂停，请更新密钥。')}</p>
        <button type="button" className="wxv-btn" onClick={() => onRecoverWechatKeys(source.account_id)}>{t('更新微信密钥')}</button>
      </div>
    ))}
    {(error || runtime.error) && <p className="wx-setup-error" role="alert">{errorText(error || runtime.error)}</p>}
    {!visible.length && <p>{t('暂无任务。完成备份设置或生成聊天总结后，会显示在这里。')}</p>}
    <div className="task-runtime-list">{visible.map(job => <article key={job.id} className="task-runtime-job"><div><strong>{taskTitle(job)}</strong><p>{taskMessage(job)}{emptyCounts.has(job.id) && <> · {tf('当天 {0} 次', emptyCounts.get(job.id)!)}</>}</p>{job.kind==='summary'&&<p className="task-summary-caller">{job.caller} · {tf('发起账号：{0}',job.initiator_id??'—')}</p>}<small>{new Date(job.updated_at).toLocaleString(dateLocale())}</small>{job.status === 'failed' && <WizardActionButton disabled={job.kind!=='summary'&&runtime.paused} onAction={async () => { if(job.kind==='summary')await invoke('summary_task_retry',{taskId:job.id});else await invoke('task_runtime_retry', { jobId: job.id }); await refreshTaskRuntime(); }}>{t('重试')}</WizardActionButton>}</div><span className={`task-status ${job.status === 'completed' ? 'done' : job.status === 'failed' ? 'failed' : 'pending'}`}>{t(job.kind==='summary'?({queued:'排队中',running:'正在生成',processing:'云端生成中',completed:'已完成',failed:'等待重试'} as Record<string,string>)[job.status]||job.status:labels[job.status] || job.status)}</span></article>)}</div>
    <div className="page-actions"><WizardActionButton onAction={() => loadCloud()}>{t('查看云端历史')}</WizardActionButton></div>
    {cloudPage > 0 && <section><h3>{t('当前空间的云端历史')}</h3>{cloud.map(batch => <article className="task-runtime-job" key={batch.id}><div><strong>{batch.source_type === 'whatsapp' ? 'WhatsApp' : t('微信')} · #{batch.id}</strong><p>{batch.error_message ? errorText(batch.error_message) : batch.status === 'completed' && batch.message_count === 0 ? t('本批未新增消息') : tf('{0} 条消息', batch.message_count)}</p></div><span>{t(labels[batch.status] || batch.status)}</span></article>)}<footer className="page-actions"><button disabled={cloudBusy || cloudPage <= 1} onClick={() => void loadCloud(cloudPage - 1)}>{t('上一页')}</button><span>{cloudPage}</span><button disabled={cloudBusy || !cloudMore} onClick={() => void loadCloud(cloudPage + 1)}>{t('下一页')}</button></footer></section>}
    {jobs.length > 20 && <footer className="page-actions"><button disabled={activePage === 0} onClick={() => setPage(activePage - 1)}>{t('上一页')}</button><span>{activePage + 1} / {Math.ceil(jobs.length / 20)}</span><button disabled={(activePage + 1) * 20 >= jobs.length} onClick={() => setPage(activePage + 1)}>{t('下一页')}</button></footer>}
  </section>;
}
