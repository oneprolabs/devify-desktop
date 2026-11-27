import { useLayoutEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../shared/api/desktopInvoke';
let activeReads = 0;
const waiting: Array<() => void> = [];
function readAvatar(run: () => Promise<void>) {
  const start = () => { activeReads += 1; void run().finally(() => { activeReads -= 1; waiting.shift()?.(); }); };
  if (activeReads < 2) start(); else waiting.push(start);
}
/** Real image data is requested only for visible rows and one screen around them. */
export function LocalChatAvatar({ accountId, username, label, avatarRevision }: { accountId: string; username: string; label: string; avatarRevision: number }) {
  const root = useRef<HTMLSpanElement>(null);
  const [src, setSrc] = useState('');
  useLayoutEffect(() => {
    const node = root.current;
    if (!node) return;
    let mounted = true; let near = false; let queued = false; let loaded = false;
    const viewport = node.closest('.chat-workbench-sessions');
    const observer = new IntersectionObserver(entries => {
      near = entries.some(entry => entry.isIntersecting);
      if (!mounted || !near || queued || loaded) return;
      queued = true;
      readAvatar(async () => {
        try {
          if (!mounted || !near) return;
          const items = await invoke<Array<{ username: string; data_url: string }>>('wechat_avatars', { accountId, usernames: [username] });
          loaded = true;
          if (mounted && items[0]?.data_url) setSrc(items[0].data_url);
        } catch { /* Initials remain readable when an image is unavailable. */ }
        finally { queued = false; }
      });
    }, { root: viewport, rootMargin: `${(viewport as HTMLElement | null)?.clientHeight ?? 0}px 0px` });
    observer.observe(node);
    return () => { mounted = false; observer.disconnect(); };
  }, [accountId, username, avatarRevision]);
  return <span ref={root}>{src ? <img src={src} alt="" /> : label.slice(0, 1)}</span>;
}
