import { createRoot } from 'react-dom/client';
import './style.css';
import { isTauri } from '@tauri-apps/api/core';
import PrototypeShell from './PrototypeShell';
import { suppressDesktopContextMenu } from './contextMenu';
import { AppErrorBoundary } from './AppErrorBoundary';
import { recordDiagnostic } from './desktopInvoke';

document.addEventListener('contextmenu', event => suppressDesktopContextMenu(event, isTauri()));

window.addEventListener('unhandledrejection', () => recordDiagnostic({ module: 'ui', operation: 'exception', code: 'UNHANDLED_REJECTION' }));
window.addEventListener('error', () => recordDiagnostic({ module: 'ui', operation: 'exception', code: 'RENDER_ERROR' }));
createRoot(document.getElementById('root')!).render(<AppErrorBoundary><PrototypeShell /></AppErrorBoundary>);
