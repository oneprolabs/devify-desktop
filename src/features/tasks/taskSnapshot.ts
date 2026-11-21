import { settingsScope, type CloudPage, type TaskSettings } from './taskCloud.ts';

type Snapshot = { scope: string; userId: number; deviceId: string; page: CloudPage };
let snapshot: Snapshot | null = null;

export function clearTaskSnapshot() { snapshot = null; }
export function saveTaskSnapshot(settings: TaskSettings, userId: number | null, deviceId: string | null, page: CloudPage) {
  if (!userId || !deviceId) { clearTaskSnapshot(); return; }
  snapshot = { scope: settingsScope(settings), userId, deviceId, page };
}
export function readTaskSnapshot(settings: TaskSettings, userId: number | null, deviceId: string | null): CloudPage | null {
  return snapshot && userId && deviceId && snapshot.scope === settingsScope(settings) && snapshot.userId === userId && snapshot.deviceId === deviceId ? snapshot.page : null;
}
