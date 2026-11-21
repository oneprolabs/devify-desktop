import { useEffect, useId, useRef, useState } from 'react';
import { t, tf } from '../../shared/i18n/locale';
import { AGENTS, agentById, type AgentInfo } from './agentCatalog';

export function AgentSelect({ value, automatic, agents, disabled, onChange }: {
  value: string; automatic: string | null; agents: AgentInfo[]; disabled: boolean; onChange: (id: string) => void;
}) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const selected = agentById(value || automatic || '');
  const options = [
    { value: '', name: t('自动选择'), icon: agentById(automatic ?? '')?.icon, note: automatic ? tf('优先使用 {0}', agentById(automatic)!.name) : t('未发现可用智能体'), disabled: !automatic },
    ...AGENTS.map(agent => ({ value: agent.id, name: agent.name, icon: agent.icon,
      note: !agents.some(item => item.id === agent.id && item.available) ? t('未安装') : !agent.callable ? t('调用待适配') : t('已安装'),
      disabled: !agent.callable || !agents.some(item => item.id === agent.id && item.available) })),
  ];
  const enabled = options.map((option, index) => !option.disabled ? index : -1).filter(index => index >= 0);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open]);
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => { if (open) document.getElementById(`${id}-${active}`)?.scrollIntoView({ block: 'nearest' }); }, [id, open, active]);
  const reveal = () => { setOpen(true); setActive(Math.max(0, options.findIndex(option => option.value === value && !option.disabled))); };
  const choose = (index: number) => { if (options[index].disabled) return; setOpen(false); onChange(options[index].value); trigger.current?.focus(); };
  return <div className="integration-select" ref={root}>
    <span id={`${id}-label`}>{t('默认智能体')}</span>
    <button ref={trigger} type="button" role="combobox" aria-haspopup="listbox" aria-expanded={open} aria-controls={`${id}-options`}
      aria-labelledby={`${id}-label ${id}-value`} aria-activedescendant={open ? `${id}-${active}` : undefined}
      disabled={disabled} onClick={() => open ? setOpen(false) : reveal()}
      onKeyDown={event => {
        if (event.key === 'Escape' || event.key === 'Tab') { setOpen(false); return; }
        if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault();
          if (!open) { reveal(); return; }
          const position = enabled.indexOf(active);
          setActive(event.key === 'Home' ? enabled[0] : event.key === 'End' ? enabled[enabled.length - 1] : enabled[(position + (event.key === 'ArrowDown' ? 1 : -1) + enabled.length) % enabled.length]);
        } else if (open && ['Enter', ' '].includes(event.key)) { event.preventDefault(); choose(active); }
      }}>
      <span id={`${id}-value`}>{selected && <img src={selected.icon} alt=""/>}<span>{value ? selected?.name ?? value : automatic ? tf('自动 · {0}', selected!.name) : t('未发现可用智能体')}</span></span>
      <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m5 7 5 5 5-5"/></svg>
    </button>
    {open && <div className="integration-select-options" id={`${id}-options`} role="listbox" aria-label={t('默认智能体')}>
      {options.map((option, index) => <div id={`${id}-${index}`} key={option.value} role="option" aria-selected={option.value === value} aria-disabled={option.disabled}
        className={index === active ? 'keyboard-active' : ''} onClick={() => choose(index)}>
        {option.icon && <img src={option.icon} alt=""/>}<span><strong>{option.name}</strong><small>{option.note}</small></span>
        {value === option.value && <svg className="integration-check" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="m4 10 4 4 8-8"/></svg>}
      </div>)}
    </div>}
  </div>;
}
