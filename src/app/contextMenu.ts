type ContextMenuEvent = { target: EventTarget | null; preventDefault: () => void };

export function suppressDesktopContextMenu(event: ContextMenuEvent, desktop: boolean) {
  if (!desktop) return;
  const target = event.target as Element | null;
  if (typeof target?.closest === 'function' && target.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
  event.preventDefault();
}
