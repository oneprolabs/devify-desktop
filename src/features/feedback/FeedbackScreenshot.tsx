import { t } from '../../shared/i18n/locale';
import { useEffect, useRef, useState, type PointerEvent } from 'react';
import { imageSource } from './feedbackTypes';

function encodeCanvas(canvas: HTMLCanvasElement): string {
  let url = canvas.toDataURL('image/png');
  if (url.length > 4 * 1024 * 1024) url = canvas.toDataURL('image/jpeg', 0.86);
  if (url.length > 4 * 1024 * 1024) throw new Error(t("截图较大，请先缩小或裁剪后添加。"));
  return url.split(',')[1];
}

export async function readScreenshot(file: File): Promise<string> {
  if (!['image/png', 'image/jpeg'].includes(file.type) || file.size > 10 * 1024 * 1024) throw new Error(t("请选择 10 MB 以内的 PNG/JPEG 截图。"));
  const url = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(t("无法读取截图。")));
    reader.readAsDataURL(file);
  });
  const img = new Image();
  img.src = url;
  await img.decode().catch(() => { throw new Error(t("截图无法打开。")); });
  if (img.width * img.height > 16000000) throw new Error(t("截图超过 1600 万像素，请裁剪后添加。"));
  const ratio = Math.min(1, 2400 / Math.max(img.width, img.height));
  const canvas = document.createElement('canvas');
  canvas.width = Math.round(img.width * ratio); canvas.height = Math.round(img.height * ratio);
  canvas.getContext('2d')!.drawImage(img, 0, 0, canvas.width, canvas.height);
  return encodeCanvas(canvas);
}

type Point = { x: number; y: number };
export function FeedbackScreenshot({ data, onSave, onCancel }: { data: string; onSave: (value: string) => void; onCancel: () => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [working, setWorking] = useState(data);
  const [start, setStart] = useState<Point | null>(null);
  const [end, setEnd] = useState<Point | null>(null);
  const [error, setError] = useState('');
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let active = true; setReady(false);
    const img = new Image(); img.src = imageSource(working);
    void img.decode().then(() => {
      if (!active || !canvas.current) return;
      canvas.current.width = img.width; canvas.current.height = img.height;
      canvas.current.getContext('2d')!.drawImage(img, 0, 0); setReady(true);
    }).catch(() => setError(t("无法显示截图。")));
    return () => { active = false; };
  }, [working]);
  const point = (event: PointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return { x: Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (event.clientY - rect.top) / rect.height)) };
  };
  const rect = start && end ? { left: Math.min(start.x, end.x), top: Math.min(start.y, end.y), width: Math.abs(start.x - end.x), height: Math.abs(start.y - end.y) } : null;
  const apply = (crop: boolean) => {
    if (!rect || !canvas.current) return;
    const source = canvas.current;
    const x = Math.floor(rect.left * source.width), y = Math.floor(rect.top * source.height);
    const width = Math.max(1, Math.floor(rect.width * source.width)), height = Math.max(1, Math.floor(rect.height * source.height));
    const target = document.createElement('canvas');
    target.width = crop ? width : source.width; target.height = crop ? height : source.height;
    const ctx = target.getContext('2d')!;
    if (crop) ctx.drawImage(source, x, y, width, height, 0, 0, width, height);
    else { ctx.drawImage(source, 0, 0); ctx.fillStyle = '#000'; ctx.fillRect(x, y, width, height); }
    try { setWorking(encodeCanvas(target)); setStart(null); setEnd(null); } catch (error) { setError(String(error)); }
  };
  return <section className="feedback-editor" aria-label={t("编辑截图")}>
    <div className="feedback-editor-toolbar"><strong>{t("裁剪与遮盖")}</strong><span>{t("拖动框选区域")}</span><button type="button" disabled={!rect || !ready} onClick={() => apply(false)}>{t("实心遮盖")}</button><button type="button" disabled={!rect || !ready} onClick={() => apply(true)}>{t("裁剪选区")}</button><button type="button" disabled={working === data} onClick={() => { setWorking(data); setStart(null); setEnd(null); }}>{t("重置")}</button></div>
    <div className="feedback-canvas-wrap"><canvas ref={canvas} aria-label={t("拖动选择截图区域")} onPointerDown={event => { if (!ready) return; event.currentTarget.setPointerCapture(event.pointerId); setStart(point(event)); setEnd(point(event)); }} onPointerMove={event => { if (event.buttons === 1 && start) setEnd(point(event)); }} onPointerUp={event => { if (start) setEnd(point(event)); }}/>{rect && <div className="feedback-selection" style={{ left: `${rect.left * 100}%`, top: `${rect.top * 100}%`, width: `${rect.width * 100}%`, height: `${rect.height * 100}%` }}/>}</div>
    {error && <p role="alert">{t(error)}</p>}
    <div className="feedback-actions"><button type="button" onClick={onCancel}>{t("取消编辑")}</button><button type="button" className="primary" disabled={!ready} onClick={() => onSave(working)}>{t("保存图片")}</button></div>
  </section>;
}
