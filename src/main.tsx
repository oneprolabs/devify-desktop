import { createRoot } from 'react-dom/client';
import './style.css';
import PrototypeShell from './PrototypeShell';
import { isTauri } from '@tauri-apps/api/core';
import { suppressDesktopContextMenu } from './contextMenu';

document.addEventListener('contextmenu', event => suppressDesktopContextMenu(event, isTauri()));

createRoot(document.getElementById('root')!).render(<PrototypeShell />);
