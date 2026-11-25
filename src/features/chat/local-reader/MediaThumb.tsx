import { useEffect, useRef, useState } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t } from '../../../shared/i18n/locale';
import { errorText } from '../../../shared/i18n/errorText';
import type { MediaItem, WhatsAppMediaRef } from './nativeTypes';

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

type Props = ({ accountId: string; item: MediaItem; whatsapp?: never; thumbnailOnly?: never }
  | { accountId?: never; item?: never; whatsapp: WhatsAppMediaRef; thumbnailOnly: boolean }) & {
    onOpen: () => void; video?: boolean; opensVideo?: boolean;
  };

export function MediaThumb({ accountId, item, whatsapp, thumbnailOnly, onOpen, video, opensVideo }: Props) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState('');
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
  }, [accountId, item?.local_id, whatsapp?.accountId, whatsapp?.localId]);
  useEffect(() => {
    if (!near || src || failed) return;
    if (item?.preview) { setSrc(convertFileSrc(item.preview)); return; }
    let cancelled = false;
    enqueuePreview(async () => {
      if (cancelled) return;
      try {
        const path = whatsapp
          ? await invoke<string>('whatsapp_attachment_action', { ...whatsapp, action: 'preview' })
          : await invoke<string | null>('wechat_media_preview', { accountId, localId: item.local_id, paths: item.sources });
        if (!cancelled) {
          if (path) setSrc(whatsapp ? path : convertFileSrc(path));
          else setFailed(t("图片加载失败"));
        }
      } catch (reason) {
        if (!cancelled) setFailed((whatsapp && errorText(reason)) || t("图片加载失败"));
      }
    });
    return () => {
      cancelled = true;
    };
  }, [accountId, item?.local_id, src, failed, near, item?.preview, whatsapp?.accountId, whatsapp?.username, whatsapp?.localId, whatsapp?.time]);

  const loadingLabel = video ? t("加载中…") : t("正在加载图片…");
  if (!src) {
    if (failed && opensVideo) {
      return <button type="button" className="wxv-attach-link" onClick={onOpen}>{t("打开")}</button>;
    }
    return (
      <span ref={node => { holder.current = node; }} className="wxv-image-placeholder" role="status">
        {!failed && <i className="wxv-image-spinner" aria-hidden="true"/>}{failed || loadingLabel}
      </span>
    );
  }
  const onlyThumb = whatsapp ? thumbnailOnly : item.sources.length > 0 && item.sources.every(path => path.endsWith('_t.dat'));
  const localId = whatsapp ? whatsapp.localId : item.local_id;
  return (
    <button
      type="button"
      className="wxv-attach-thumb"
      ref={node => { holder.current = node; }}
      onClick={onOpen}
      title={opensVideo ? t("打开") : onlyThumb ? t(whatsapp ? "本机只有缩略图" : "本机只有缩略图（微信未下载原图）") : video ? t("视频封面") : t("查看原图")}
      data-open-image={localId}
    >
      {!loaded && <span className="wxv-image-placeholder" role="status"><i className="wxv-image-spinner" aria-hidden="true"/>{loadingLabel}</span>}
      <img src={src} alt={video ? t("视频封面") : t("图片")} data-open-image={localId} onLoad={() => { setLoaded(true); }} onError={() => { setSrc(null); setFailed(t("图片加载失败")); }} style={loaded ? undefined : { position: 'absolute', visibility: 'hidden' }} />
      {(onlyThumb || video) && <span className="wxv-attach-badge">{onlyThumb ? t("仅缩略图") : t("视频")}</span>}
    </button>
  );
}
