const snapshots = new Map<string, unknown>();
let generation = 0;

export function pageSnapshotGeneration() { return generation; }

export function readPageSnapshot<T>(key: string): T | undefined {
  return snapshots.get(key) as T | undefined;
}

export function savePageSnapshot<T>(key: string, value: T, expectedGeneration = generation) {
  if (expectedGeneration !== generation) return;
  snapshots.delete(key);
  snapshots.set(key, value);
  if (snapshots.size > 30) {
    const oldest = snapshots.keys().next().value;
    if (oldest !== undefined) snapshots.delete(oldest);
  }
}

export function clearPageSnapshots() { generation += 1; snapshots.clear(); }
