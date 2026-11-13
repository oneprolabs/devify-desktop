import { useEffect, useRef, useState } from 'react';
import { stepTimePart } from './dailyPlan';

type Part = 'hour' | 'minute';
function TimeWheel({ value, part, onChange }: { value: string; part: Part; onChange: (time: string) => void }) {
  const wheel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = wheel.current;
    if (!node) return;
    const scroll = (event: WheelEvent) => {
      if (!event.deltaY) return;
      event.preventDefault();
      onChange(stepTimePart(value, part, event.deltaY > 0 ? 1 : -1));
    };
    node.addEventListener('wheel', scroll, { passive: false });
    return () => node.removeEventListener('wheel', scroll);
  }, [value, part, onChange]);
  const index = part === 'hour' ? 0 : 1;
  const label = part === 'hour' ? '小时' : '分钟';
  const display = (step: number) => stepTimePart(value, part, step).split(':')[index];
  return <div className="daily-wheel-column"><span className="daily-wheel-label">{label}</span>
    <div ref={wheel} className="daily-wheel" role="group" aria-label={`${label}滚动选择`}>
      {[-1, 0, 1].map(step => <button key={step} type="button"
        className={step === 0 ? 'daily-wheel-current' : ''}
        aria-label={step === 0 ? `当前${label} ${display(step)}` : `${step < 0 ? '上一个' : '下一个'}${label} ${display(step)}`}
        onClick={() => { if (step) onChange(stepTimePart(value, part, step)); }}>{display(step)}</button>)}
    </div>
  </div>;
}

export function DailyTimePicker({ value, disabled, onChange, onConfirm, onCancel }: { value: string; disabled: boolean; onChange: (time: string) => void; onConfirm: () => void; onCancel: () => void }) {
  const [open, setOpen] = useState(false);
  return <div className="daily-picker-wrap">
    <button type="button" className="daily-picker-trigger" aria-label={`日报时间 ${value}`} disabled={disabled} aria-expanded={open} onClick={() => { if (open) onConfirm(); setOpen(current => !current); }}>
      <span aria-hidden="true">◷</span><strong>{value}</strong><span className="daily-picker-chevron" aria-hidden="true">⌄</span>
    </button>
    {open && !disabled && <div className="daily-picker-panel" onKeyDown={event => { if (event.key === 'Escape') { setOpen(false); onCancel(); } }}>
      <div className="daily-picker-title">选择时间 <span>滚动鼠标选择小时和分钟</span></div>
      <div className="daily-picker-wheels"><TimeWheel value={value} part="hour" onChange={onChange}/><span className="daily-picker-colon">:</span><TimeWheel value={value} part="minute" onChange={onChange}/></div>
      <button type="button" className="daily-picker-done" onClick={() => { setOpen(false); onConfirm(); }}>完成</button>
    </div>}
  </div>;
}
