import { t } from './locale';
/**
 * One shared, self-dismissing toast for placeholder actions.
 *
 * Export is intentionally left empty for now: every entry point keeps its button but only
 * reports that the feature is not available yet. A toast keeps that feedback identical in
 * the chat pane, the workbench header and the report page.
 */
let hideTimer: number | undefined;

export function toast(message: string): void {
  if (typeof document === 'undefined') return;
  let node = document.getElementById('app-toast');
  if (!node) {
    node = document.createElement('div');
    node.id = 'app-toast';
    node.setAttribute('role', 'status');
    node.setAttribute('aria-live', 'polite');
    node.className = 'app-toast';
    document.body.appendChild(node);
  }
  node.textContent = message;
  node.classList.add('on');
  if (hideTimer) window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => node?.classList.remove('on'), 2600);
}

/** Feedback for every export button while the feature is a placeholder. */
export function exportNotSupported(): void {
  toast(t("导出功能暂不支持，后续版本开放。"));
}
