import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import type { SummaryConfigError, SummaryDraft, SummaryOptionsView } from './summaryOptions';

export function useSummaryOptions(scope: string, open: boolean) {
  const [read, setRead] = useState<{ scope: string; view: SummaryOptionsView | null; loading: boolean; error: SummaryConfigError | null } | null>(null);
  const [draft, setDraft] = useState<SummaryDraft>({ scenario: '', language: '', instructions: '' });
  const epoch = useRef(0);
  const active = useRef(false);
  const initialized = useRef('');
  const current = useRef(scope); current.current = scope;
  const lock = useRef<number | null>(null);
  useEffect(() => { active.current = true; return () => { active.current = false; epoch.current++; lock.current = null; }; }, []);
  useEffect(() => { epoch.current++; lock.current = null; initialized.current = ''; setRead(null); setDraft({ scenario: '', language: '', instructions: '' }); }, [scope]);
  const load = useCallback(async () => {
    if (lock.current !== null) return;
    const owner = scope, request = ++epoch.current;
    lock.current = request;
    const valid = () => active.current && current.current === owner && epoch.current === request;
    setRead(old => ({ scope: owner, view: old?.scope === owner ? old.view : null, loading: true, error: null }));
    try {
      const next = await invoke<SummaryOptionsView>('summary_config_get');
      if (!valid()) return;
      setRead({ scope: owner, view: next, loading: false, error: null });
      const first = initialized.current !== owner;
      setDraft(old => first
        ? { scenario: next.config.default_scenario, language: '', instructions: next.instructions || '' }
        : { ...old, scenario: next.config.scenarios.some(item => item.id === old.scenario) ? old.scenario : next.config.default_scenario });
      initialized.current = owner;
    } catch (reason) {
      if (!valid()) return;
      const error = reason && typeof reason === 'object' && 'message' in reason && typeof reason.message === 'string'
        ? { message: reason.message, ...('retry_at' in reason && typeof reason.retry_at === 'number' ? { retry_at: reason.retry_at } : {}) }
        : { message: String(reason) };
      setRead(old => ({ scope: owner, view: old?.scope === owner ? old.view : null, loading: false, error }));
    } finally { if (lock.current === request) lock.current = null; }
  }, [scope]);
  useEffect(() => { if (open) void load(); }, [open, load]);
  const activeRead = read?.scope === scope ? read : null;
  return { view: activeRead?.view ?? null, draft, setDraft, loading: activeRead?.loading ?? open, error: activeRead?.error ?? null, reload: load };
}
