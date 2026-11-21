import { useCallback, useEffect, useRef, useState } from 'react';
import { t, tf } from './locale';
import './chat-platform-bar.css';

export type ChatPlatform = { id: string; label: string; icon: string; updated: boolean };

/** Navigation only: selecting a platform never changes connector or backup settings. */
export function ChatPlatformBar({ platforms, selected, onSelect, onAddPlatform, visible = true }: {
  platforms: ChatPlatform[]; selected: string; onSelect: (source: string) => void; onAddPlatform?: () => void; visible?: boolean;
}) {
  const track = useRef<HTMLDivElement>(null);
  const timer = useRef<number>();
  const [edges, setEdges] = useState({ left: false, right: false, updates: 0 });
  const measure = useCallback(() => {
    const node = track.current;
    if (!node) return;
    const bounds = node.getBoundingClientRect();
    const updates = [...node.querySelectorAll<HTMLElement>('[data-platform-updated="true"]')].filter(button => {
      const rect = button.getBoundingClientRect();
      return rect.left < bounds.left - 1 || rect.right > bounds.right + 1;
    }).length;
    const next = { left: node.scrollLeft > 1, right: node.scrollLeft + node.clientWidth < node.scrollWidth - 1, updates };
    setEdges(current => current.left === next.left && current.right === next.right && current.updates === next.updates ? current : next);
  }, []);
  const stop = useCallback(() => { window.clearTimeout(timer.current); timer.current = undefined; }, []);
  const move = (amount: number) => { track.current?.scrollBy({ left: amount, behavior: 'auto' }); };
  const start = (direction: number) => {
    stop();
    if (!visible || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const tick = () => {
      const node = track.current;
      if (!node) return;
      const before = node.scrollLeft;
      node.scrollLeft += direction * 8;
      if (Math.abs(node.scrollLeft - before) < 1) { stop(); return; }
      timer.current = window.setTimeout(tick, 32);
    };
    tick();
  };
  useEffect(() => {
    const node = track.current;
    if (!node) return;
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    // Native listener allows consuming a wheel only when this strip can move.
    const wheel = (event: WheelEvent) => {
      if (event.ctrlKey) return;
      stop();
      const before = node.scrollLeft;
      node.scrollLeft += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (node.scrollLeft !== before) event.preventDefault();
    };
    const hide = () => { if (document.hidden) stop(); };
    node.addEventListener('wheel', wheel, { passive: false });
    window.addEventListener('blur', stop);
    document.addEventListener('visibilitychange', hide);
    measure();
    return () => {
      stop(); observer.disconnect(); node.removeEventListener('wheel', wheel);
      window.removeEventListener('blur', stop); document.removeEventListener('visibilitychange', hide);
    };
  }, [measure, stop]);
  useEffect(() => {
    if (!visible) stop();
    const node = track.current;
    const button = [...node?.querySelectorAll<HTMLButtonElement>('[role="tab"]') ?? []].find(item => item.dataset.platform === selected);
    if (node && button) {
      const bounds = node.getBoundingClientRect(), rect = button.getBoundingClientRect();
      if (rect.left < bounds.left || rect.right > bounds.right) node.scrollLeft += rect.left - bounds.left - (bounds.width - rect.width) / 2;
    }
    measure();
  }, [platforms, selected, visible, stop, measure]);
  if (!platforms.length) return null;
  const single = platforms.length === 1 && Boolean(onAddPlatform);
  return <div className={`chat-platform-bar${single ? ' is-single' : ''}`}>
    <div className="chat-platform-meta">
      <span>{tf("{0} 个平台", platforms.length)}</span>
      <span className="chat-platform-updates" role="status">{edges.updates ? tf("还有 {0} 个平台有更新", edges.updates) : ''}</span>
    </div>
    <div className="chat-platform-rail">
      <button type="button" className="chat-platform-arrow" aria-label={t("向左查看平台")} disabled={!edges.left}
        onPointerEnter={event => { if (event.pointerType === 'mouse') start(-1); }} onPointerLeave={stop} onPointerCancel={stop} onBlur={stop} onClick={() => { stop(); move(-144); }}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 5-5 5 5 5"/></svg>
      </button>
      <div className="chat-platform-track" ref={track} role="tablist" aria-label={t("聊天平台")} onScroll={measure}
        onKeyDown={event => {
          if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
          event.preventDefault(); stop();
          const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : Math.max(0, Math.min(buttons.length - 1, index + (event.key === 'ArrowRight' ? 1 : -1)));
          buttons[next]?.focus();
        }}>
        {platforms.map(platform => <button type="button" role="tab" key={platform.id} data-platform={platform.id}
          data-platform-updated={platform.updated} className={`chat-platform-tab${platform.id === selected ? ' active' : ''}`}
          aria-selected={platform.id === selected} aria-label={platform.updated ? tf("{0}，有更新", platform.label) : platform.label}
          title={platform.label} tabIndex={platform.id === selected ? 0 : -1} onClick={() => { stop(); onSelect(platform.id); }}>
          <img src={platform.icon} alt="" width={24} height={24}/>
          <span>{platform.label}</span>
          {platform.updated && <i className="chat-platform-dot" aria-hidden="true"/>}
        </button>)}
      </div>
      <button type="button" className="chat-platform-arrow" aria-label={t("向右查看更多平台")} disabled={!edges.right}
        onPointerEnter={event => { if (event.pointerType === 'mouse') start(1); }} onPointerLeave={stop} onPointerCancel={stop} onBlur={stop} onClick={() => { stop(); move(144); }}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m8 5 5 5-5 5"/></svg>
      </button>
      {single && <button type="button" className="chat-platform-add" onClick={() => { stop(); onAddPlatform?.(); }}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4v12M4 10h12"/></svg><span>{t("添加平台")}</span>
      </button>}
    </div>
  </div>;
}
