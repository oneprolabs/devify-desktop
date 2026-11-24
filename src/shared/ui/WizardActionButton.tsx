import { useEffect, useRef, useState, type ButtonHTMLAttributes } from 'react';
import { t, tf } from '../i18n/locale';
import { errorText } from '../i18n/errorText';

type Props = Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'onClick'> & {
  onAction: () => unknown | Promise<unknown>;
  loading?: boolean;
  busyLabel?: string;
};

// Let the loading state paint before native IPC. The one-shot fallback also
// releases the action when an inactive WebView throttles animation frames.
function afterPaint() {
  if (typeof window.requestAnimationFrame !== 'function') return Promise.resolve();
  return new Promise<void>(resolve => {
    const timer = window.setTimeout(resolve, 100);
    window.requestAnimationFrame(() => window.requestAnimationFrame(() => {
      window.clearTimeout(timer); resolve();
    }));
  });
}

export function WizardActionButton({ onAction, loading = false, busyLabel, disabled, children, className = '', ...props }: Props) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const locked = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const active = loading || pending;

  return <><button {...props} type={props.type ?? 'button'} className={`wizard-action ${className}`} disabled={disabled || active} aria-busy={active}
    onClick={async event => {
      if (locked.current || disabled || loading) return;
      locked.current = true;
      event.currentTarget.disabled = true;
      setPending(true); setError('');
      try {
        await afterPaint();
        if (mounted.current) await onAction();
      } catch (reason) {
        if (mounted.current) setError(String(reason));
      } finally {
        locked.current = false;
        if (mounted.current) setPending(false);
      }
    }}>
    {active && <svg className="wizard-button-spinner" viewBox="0 0 16 16" fill="none" aria-hidden="true"><circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="1.8" opacity=".25" /><path d="M8 2a6 6 0 0 1 6 6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" /></svg>}
    <span>{active ? busyLabel ?? t("正在处理…") : children}</span>
  </button>{error && <p className="wx-setup-error" role="alert">{tf("操作未完成：{0} 请重试。", errorText(error))}</p>}</>;
}
