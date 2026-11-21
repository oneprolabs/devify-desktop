import { t, tf, ctf } from '../i18n/locale';
/** Sequentially save explicit user edits against the last confirmed user revision. */
export type SettingsEdit<T> = (current: T) => T;

export type SettingsSaveSnapshot<T> = {
  draft: T;
  pending: number;
  saving: boolean;
  blocked: boolean;
  error: unknown | null;
};

/** Missing or unsafe revisions must never be treated as revision zero. */
export function settingsSaveFailure(reason: unknown): string {
  const message = reason instanceof Error ? reason.message : String(reason);
  if (message.includes('SETTINGS_CONFLICT:')) {
    return t("设置已在其他页面或账号、空间中更改；本页修改尚未保存，请核对当前账号和空间。");
  }
  return tf("保存失败，修改尚未确认：{0}", message);
}

export function settingsUserRevision(value: unknown): number {
  const revision = (value as { user_settings_revision?: unknown } | null)?.user_settings_revision;
  if (typeof revision !== 'number' || !Number.isSafeInteger(revision) || revision < 0) {
    throw new Error(t("设置缺少有效版本，无法安全保存；请更新应用并重新加载。"));
  }
  return revision;
}

export function createSettingsSaveQueue<T>(options: {
  initial: T;
  save: (value: T) => Promise<T>;
  /** Runs only after a confirmed save; a failure does not mark that save as failed. */
  afterSave?: (value: T) => Promise<void>;
  onAfterSaveError?: (error: unknown) => void;
}) {
  settingsUserRevision(options.initial);
  let confirmed = options.initial;
  let pending: SettingsEdit<T>[] = [];
  let saving = false;
  let blocked = false;
  let error: unknown | null = null;
  let listener: ((state: SettingsSaveSnapshot<T>) => void) | null = null;

  const snapshot = (): SettingsSaveSnapshot<T> => ({
    draft: pending.reduce((value, edit) => edit(value), confirmed),
    pending: pending.length,
    saving,
    blocked,
    error,
  });
  const emit = () => listener?.(snapshot());

  const run = async () => {
    if (saving || blocked || !pending.length) return;
    saving = true;
    emit();
    while (pending.length && !blocked) {
      const edit = pending[0];
      try {
        const expectedRevision = settingsUserRevision(confirmed);
        const value = edit(confirmed);
        if (settingsUserRevision(value) !== expectedRevision) {
          throw new Error(t("保存操作不能修改设置版本。"));
        }
        const saved = await options.save(value);
        if (settingsUserRevision(saved) < expectedRevision) {
          throw new Error(t("保存返回了旧版本，无法确认设置；请重新加载。"));
        }
        confirmed = saved;
        pending = pending.slice(1);
        emit();
      } catch (reason) {
        // Keep all edits visible, stop the queue, and require an explicit reload.
        error = reason;
        blocked = true;
        break;
      }
      if (options.afterSave) {
        try {
          await options.afterSave(confirmed);
        } catch (reason) {
          options.onAfterSaveError?.(reason);
        }
      }
    }
    saving = false;
    emit();
  };

  return {
    snapshot,
    subscribe(next: (state: SettingsSaveSnapshot<T>) => void) {
      listener = next;
      next(snapshot());
      return () => { if (listener === next) listener = null; };
    },
    enqueue(edit: SettingsEdit<T>): boolean {
      if (blocked) return false;
      pending.push(edit);
      emit();
      void run();
      return true;
    },
  };
}

export type SettingsSaveQueue<T> = ReturnType<typeof createSettingsSaveQueue<T>>;
