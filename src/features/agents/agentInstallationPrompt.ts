import { invoke } from '@tauri-apps/api/core';

// Start the clipboard write during the click. WebKit requires this even when the
// native app must first prepare the text asynchronously.
export async function copyInstallationPrompt(): Promise<void> {
  const pending = invoke<string>('agent_access_installation_prompt');
  if (typeof ClipboardItem !== 'undefined' && typeof navigator.clipboard?.write === 'function') {
    const content = pending.then(text => new Blob([text], { type: 'text/plain' }));
    await Promise.all([pending, navigator.clipboard.write([new ClipboardItem({ 'text/plain': content })])]);
  } else {
    const text = await pending;
    await navigator.clipboard.writeText(text);
  }
}
