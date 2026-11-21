import { t } from '../i18n/locale';
import { useRef, useState, type KeyboardEvent, type PointerEvent } from 'react';

const MIN_WIDTH = 280;
const MIN_CONTENT = 350;

export function SidebarResizeHandle() {
  const [width, setWidth] = useState<number | undefined>();
  const drag = useRef<{ startX: number; startWidth: number } | null>(null);

  const setSidebarWidth = (handle: HTMLElement, requested: number) => {
    const sidebar = handle.parentElement;
    const layout = sidebar?.parentElement;
    if (!sidebar || !layout) return;
    const maxWidth = Math.max(MIN_WIDTH, layout.clientWidth - MIN_CONTENT);
    const next = Math.round(Math.max(MIN_WIDTH, Math.min(maxWidth, requested)));
    layout.style.setProperty('--sidebar-width', `${next}px`);
    setWidth(next);
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const sidebar = event.currentTarget.parentElement;
    if (!sidebar) return;
    drag.current = { startX: event.clientX, startWidth: sidebar.getBoundingClientRect().width };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (drag.current) setSidebarWidth(event.currentTarget, drag.current.startWidth + event.clientX - drag.current.startX);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const current = event.currentTarget.parentElement?.getBoundingClientRect().width;
    if (current != null) setSidebarWidth(event.currentTarget, current + (event.key === 'ArrowLeft' ? -10 : 10));
  };

  return <div className="sidebar-resize-handle" role="separator" aria-label={t("调整列表宽度")} aria-orientation="vertical" aria-valuemin={MIN_WIDTH} aria-valuenow={width} tabIndex={0} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} onKeyDown={onKeyDown}/>;
}
