import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import type { SummaryDraft, SummaryOptionsView } from './summaryOptions';

/** Reads the native cache only. Explicit initialization is the sole UI network action. */
export function useSummaryOptions(scope: string, open: boolean) {
  const [view, setView] = useState<SummaryOptionsView | null>(null);
  const [draft, setDraft] = useState<SummaryDraft>({ scenario: '', language: '', instructions: '' });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const epoch = useRef(0);
  const active = useRef(false);
  const initialized = useRef('');
  const current = useRef(scope); current.current = scope;
  const lock = useRef<number | null>(null);
  useEffect(() => { active.current = true; return () => { active.current = false; epoch.current++; lock.current = null; }; }, []);
  useEffect(() => { epoch.current++; lock.current = null; initialized.current = ''; setView(null); setError(''); setDraft({ scenario: '', language: '', instructions: '' }); }, [scope]);
  const load = useCallback(async (refresh = false) => {
    if (lock.current !== null) return;
    const owner = scope, request = ++epoch.current;
    lock.current = request;
    const valid = () => active.current && current.current === owner && epoch.current === request;
    setLoading(true); setError('');
    try {
      if (refresh) await invoke('summary_config_refresh');
      const next = await invoke<SummaryOptionsView>('summary_config_get');
      if (!valid()) return;
      setView(next);
      const first = initialized.current !== owner;
      setDraft(old => first
        ? { scenario: next.config.default_scenario, language: '', instructions: next.instructions || '' }
        : { ...old, scenario: next.config.scenarios.some(item => item.id === old.scenario) ? old.scenario : next.config.default_scenario });
      initialized.current = owner;
    } catch (reason) { if (valid()) setError(String(reason)); }
    finally { if (lock.current === request) lock.current = null; if (valid()) setLoading(false); }
  }, [scope]);
  useEffect(() => { if (open) void load(); }, [open, load]);
  return { view, draft, setDraft, loading, error, reload: () => load(), initialize: () => load(true) };
}
