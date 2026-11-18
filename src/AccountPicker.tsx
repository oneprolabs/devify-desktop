import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { ChatAccount } from './connectorDetect';
import wechatIcon from './assets/connectors/wechat.svg';
import whatsappIcon from './assets/connectors/whatsapp.svg';
import './account-picker.css';

const icons: Record<string, string> = { wechat: wechatIcon, whatsapp: whatsappIcon };
const labels: Record<string, string> = { wechat: '微信', whatsapp: 'WhatsApp' };

const sourceLabel = (source?: string) => labels[source ?? ''] ?? '聊天';
/** `8615652425800@s.whatsapp.net` reads better as the bare number in a narrow picker. */
const shortId = (id: string) => id.replace(/@s\.whatsapp\.net$/, '');
const sourceIcon = (source?: string) => icons[source ?? ''] ?? wechatIcon;

/**
 * Account switcher for the pages that read several local sources.
 *
 * A native `<select>` cannot show the platform logo next to the account name, and macOS
 * renders a long JID as an unstyled strip. This keeps the logo, the friendly name and the
 * raw identifier on one line while staying keyboard accessible.
 */
export function AccountPicker({ accounts, value, onChange, disabled, label = '聊天账号', nameFor }: {
  accounts: ChatAccount[];
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
  label?: string;
  nameFor?: (account: ChatAccount) => string | undefined;
}) {
  const [open, setOpen] = useState(false);
  const [focused, setFocused] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const current = accounts.find(account => account.id === value) ?? accounts[0];
  const name = (account: ChatAccount) => nameFor?.(account) || account.display || account.id;

  useEffect(() => {
    if (!open) return;
    setFocused(Math.max(0, accounts.findIndex(account => account.id === value)));
    const close = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', escape);
    };
  }, [open, accounts, value]);

  if (!accounts.length) return null;

  const choose = (id: string) => {
    setOpen(false);
    if (id !== value) onChange(id);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!open) { setOpen(true); return; }
      const step = event.key === 'ArrowDown' ? 1 : -1;
      setFocused(index => (index + step + accounts.length) % accounts.length);
      return;
    }
    if (!open) {
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setOpen(true); }
      return;
    }
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      choose(accounts[focused]?.id ?? value);
    }
  };

  return <div className="account-picker" ref={root}>
    <button
      type="button"
      className={`account-picker-trigger${open ? ' open' : ''}`}
      aria-label={label}
      aria-haspopup="listbox"
      aria-expanded={open}
      disabled={disabled}
      onClick={() => setOpen(current => !current)}
      onKeyDown={onKeyDown}
    >
      <span className="account-picker-icon" aria-hidden="true"><img src={sourceIcon(current?.source)} alt="" /></span>
      <span className="account-picker-text">
        <strong>{current ? name(current) : '未选择账号'}</strong>
        <small>{current ? `${sourceLabel(current.source)} · ${shortId(current.id)}` : '没有可用的聊天来源'}</small>
      </span>
      <svg className="account-picker-chevron" viewBox="0 0 12 12" aria-hidden="true"><path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" /></svg>
    </button>
    {open && <div className="account-picker-menu" role="listbox" aria-label={label}>
      {accounts.map((account, index) => <button
        key={account.id}
        type="button"
        role="option"
        aria-selected={account.id === value}
        className={`account-picker-option${account.id === value ? ' selected' : ''}${index === focused ? ' focused' : ''}`}
        onMouseEnter={() => setFocused(index)}
        onClick={() => choose(account.id)}
      >
        <span className="account-picker-icon" aria-hidden="true"><img src={sourceIcon(account.source)} alt="" /></span>
        <span className="account-picker-option-text"><strong>{name(account)}</strong><small>{sourceLabel(account.source)} · {shortId(account.id)}</small></span>
        {account.id === value && <svg className="account-picker-check" viewBox="0 0 14 14" aria-hidden="true"><path d="m3 7.4 2.7 2.6L11 4.4" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>}
      </button>)}
    </div>}
  </div>;
}
