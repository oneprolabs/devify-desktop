import { createRoot } from 'react-dom/client';
import './app/styles/style.css';
import { isTauri } from '@tauri-apps/api/core';
import PrototypeShell from './app/PrototypeShell';
import { suppressDesktopContextMenu } from './app/contextMenu';
import { AppErrorBoundary } from './app/AppErrorBoundary';
import { recordDiagnostic } from './shared/api/desktopInvoke';

document.addEventListener('contextmenu', event => suppressDesktopContextMenu(event, isTauri()));

window.addEventListener('unhandledrejection', () => recordDiagnostic({ module: 'ui', operation: 'exception', code: 'UNHANDLED_REJECTION' }));
window.addEventListener('error', () => recordDiagnostic({ module: 'ui', operation: 'exception', code: 'RENDER_ERROR' }));
createRoot(document.getElementById('root')!).render(<AppErrorBoundary><PrototypeShell /></AppErrorBoundary>);
