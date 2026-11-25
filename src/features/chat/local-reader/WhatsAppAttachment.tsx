import { useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { formatSize } from '../../../shared/content/localDataFormat';
import { errorText } from '../../../shared/i18n/errorText';
import { t, tf } from '../../../shared/i18n/locale';
import { MediaThumb } from './MediaThumb';
import type { WhatsAppAttachmentInfo, WhatsAppMediaRef } from './nativeTypes';

export function WhatsAppAttachment({ attachment, message, onImageOpen }: {
  attachment?: WhatsAppAttachmentInfo;
  message: WhatsAppMediaRef;
  onImageOpen: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState('');
  const status = attachment?.status ?? 'unavailable';
  const available = status === 'available';
  const kind = attachment?.kind ?? 'file';
  const video = kind === 'video';
  const showThumb = (available || status === 'thumbnail_only') && (kind === 'image' || (video && attachment?.has_thumbnail));
  const openVideo = video && available && attachment?.can_open;
  const unavailable = status === 'denied' ? t("没有权限读取此附件") :
    status === 'unsupported' ? t("此 WhatsApp 版本的附件暂不支持") : t("附件在本机不可用");
  const act = async (action: 'open' | 'save') => {
    setBusy(true);
    setError('');
    setSaved('');
    try {
      const path = await invoke<string>('whatsapp_attachment_action', { ...message, action });
      if (action === 'save') setSaved(path);
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  };
  const fileName = <>📄 {attachment?.name || t("文件")}</>;
  return (
    <div className={`wxv-attach ${kind}${kind === 'file' ? ' whatsapp-file' : ''}`} onClick={event => event.stopPropagation()}>
      {showThumb ? (
        <MediaThumb key={status} whatsapp={message} thumbnailOnly={status === 'thumbnail_only'}
          video={video} opensVideo={openVideo} onOpen={openVideo ? () => { if (!busy) void act('open'); } : onImageOpen} />
      ) : kind === 'file' ? (
        available ? (
          <details>
            <summary className="wxv-attach-chip">{fileName}</summary>
            <div className="wxv-file-details">
              <span className="wxv-attach-meta">
                {[attachment?.file_type?.toUpperCase(), attachment?.size ? formatSize(attachment.size) : ''].filter(Boolean).join(' · ')}
              </span>
              {attachment?.can_open && (
                <button type="button" className="wxv-attach-link" onClick={() => void act('open')} disabled={busy}>{t("打开")}</button>
              )}
              <button type="button" className="wxv-attach-link" onClick={() => void act('save')} disabled={busy}>{t("保存文件")}</button>
            </div>
          </details>
        ) : (
          <div className="wxv-attach-chip wxv-file-unavailable">
            <span>{fileName}</span>
            <small role="status">{unavailable}</small>
          </div>
        )
      ) : (
        <>
          {video && available && <span className="wxv-attach-chip">{t("视频")}</span>}
          {!available && <span className="wxv-attach-chip" role="status">{unavailable}</span>}
          {openVideo && (
            <button type="button" className="wxv-attach-link" onClick={() => void act('open')} disabled={busy}>{t("打开")}</button>
          )}
        </>
      )}
      {error && <p className="wxv-attach-error" role="alert">{error}</p>}
      {saved && <p className="wxv-attach-text" role="status">{tf("文件已保存到：{0}", saved)}</p>}
    </div>
  );
}
