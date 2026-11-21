import { useEffect, useRef, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t } from '../../../shared/i18n/locale';
import type { MediaItem } from './nativeTypes';

/* Decoding a WxAM original runs ffmpeg, so previews are built lazily and at most a few at
   a time. The backend caches the result on disk; this queue only throttles the IPC calls. */
const previewQueue: Array<() => void> = [];
let previewActive = 0;
const PREVIEW_CONCURRENCY = 3;
function enqueuePreview(task: () => Promise<void>) {
  previewQueue.push(() => {
    previewActive += 1;
    void task().finally(() => {
      previewActive -= 1;
      const next = previewQueue.shift();
      if (next) next();
    });
  });
  if (previewActive < PREVIEW_CONCURRENCY) {
    const next = previewQueue.shift();
    if (next) next();
  }
}

export function MediaThumb({ accountId, item, onOpen }: { accountId: string; item: MediaItem; onOpen: () => void }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const holder = useRef<HTMLElement | null>(null);
  const [near, setNear] = useState(false);
  useEffect(() => {
    const node = holder.current;
    if (!node) return;
    const root = node.closest('.wxv-messages');
    const margin = Math.max(0, (root as HTMLElement | null)?.clientHeight ?? 0);
    const observer = new IntersectionObserver(entries => { if (entries.some(entry => entry.isIntersecting)) { setNear(true); observer.disconnect(); } }, { root, rootMargin: `${margin}px 0px` });
    observer.observe(node);
    return () => observer.disconnect();
  }, [accountId, item.local_id]);
  useEffect(() => {
    if (!near || src || failed) return;
    if (item.preview) { setSrc(convertFileSrc(item.preview)); return; }
    let cancelled = false;
    enqueuePreview(async () => {
      if (cancelled) return;
      try {
        const path = await invoke<string | null>('wechat_media_preview', {
          accountId,
          localId: item.local_id,
          paths: item.sources,
        });
        if (!cancelled) {
          if (path) setSrc(convertFileSrc(path));
          else setFailed(true);
        }
      } catch {
        if (!cancelled) setFailed(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [accountId, item.local_id, src, failed, near, item.preview]);

  if (!src) {
    return (
      <span ref={node => { holder.current = node; }} className="wxv-image-placeholder" role="status">
        {!failed && <i className="wxv-image-spinner" aria-hidden="true"/>}{failed ? t("图片加载失败") : t("正在加载图片…")}
      </span>
    );
  }
  const onlyThumb = item.sources.length > 0 && item.sources.every(path => path.endsWith('_t.dat'));
  return (
    <button
      type="button"
      className="wxv-attach-thumb"
      ref={node => { holder.current = node; }}
      onClick={onOpen}
      title={onlyThumb ? t("本机只有缩略图（微信未下载原图）") : t("查看原图")}
      data-open-image={item.local_id}
    >
      {!loaded && <span className="wxv-image-placeholder" role="status"><i className="wxv-image-spinner" aria-hidden="true"/>{t("正在加载图片…")}</span>}
      <img src={src} alt={t("图片")} data-open-image={item.local_id} onLoad={() => setLoaded(true)} onError={() => { setSrc(null); setFailed(true); }} style={loaded ? undefined : { position: 'absolute', visibility: 'hidden' }} />
      {onlyThumb && <span className="wxv-attach-badge">{t("仅缩略图")}</span>}
    </button>
  );
}
