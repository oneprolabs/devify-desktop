import { useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../desktopInvoke';
import { useTaskRuntime, refreshTaskRuntime } from '../tasks/taskRuntime';
import { t, tf } from '../../locale';
import './backup-runtime-switch.css';

export function BackupRuntimeSwitch({ showLabel = false, onError }: { showLabel?: boolean; onError: (message: string) => void }) {
  const runtime = useTaskRuntime();
  const [pending, setPending] = useState(false);
  const locked = useRef(false);
  const mounted = useRef(true);
  const context = useRef(runtime.context);
  context.current = runtime.context;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const toggle = async () => {
    if (locked.current) return;
    locked.current = true;
    const owner = context.current;
    setPending(true); onError('');
    try {
      await invoke('task_runtime_pause', { paused: !runtime.paused });
      await refreshTaskRuntime();
    } catch (reason) {
      if (mounted.current && owner === context.current) onError(tf('操作未完成：{0}', String(reason)));
    } finally {
      locked.current = false;
      if (mounted.current) setPending(false);
    }
  };
  return <div className="backup-runtime-control">
    {showLabel && <span>{t('后台备份')}</span>}
    <button type="button" role="switch" aria-label={t('后台备份')} aria-checked={!runtime.paused} aria-busy={pending} disabled={pending}
      className={`backup-runtime-switch${runtime.paused ? '' : ' on'}`} onClick={() => void toggle()}><span /></button>
  </div>;
}
