import { t, tf, ctf } from '../../locale';
import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

/** One gateable capability, as described by the Rust registry. */
export type Capability = {
  key: string;
  group: string;
  label: string;
  description: string;
  /** Part of the free baseline: still usable after a paid entitlement lapses. */
  free: boolean;
};

export type Plan = {
  slug: string;
  name: string;
  status: string;
  current_period_end: string | null;
};

/**
 * What the server says this account may use. `enforced === false` (or no snapshot at all)
 * means the app behaves exactly as it did before subscriptions existed: everything works.
 */
export type Entitlements = {
  plan: Plan | null;
  features: string[];
  limits: Record<string, number>;
  quotas: Record<string, number>;
  usage: Record<string, number>;
  enforced: boolean;
  source: 'server' | 'unsupported' | 'unknown' | string;
  base_url: string;
  fetched_at: number | null;
  expires_at: string | null;
  stale: boolean;
  grace_expired: boolean;
  fallback_features: string[];
  free_keys: string[];
  /** The account holds an active subscription to the operator's internal plan. */
  internal: boolean;
};

let cached: Entitlements | null = null;
let inflight: Promise<Entitlements | null> | null = null;
const listeners = new Set<(view: Entitlements | null) => void>();

function publish(view: Entitlements | null) {
  cached = view;
  listeners.forEach(listener => listener(view));
}

/** `ingest.*` matches any key under `ingest`; `*` matches everything; otherwise exact. */
export function matchesFeature(pattern: string, key: string): boolean {
  if (pattern === '*') return true;
  if (pattern.endsWith('.*')) {
    const prefix = pattern.slice(0, -2);
    return key.startsWith(`${prefix}.`);
  }
  return pattern === key;
}

function matchesAny(list: string[], key: string): boolean {
  return list.some(pattern => matchesFeature(pattern, key));
}

/**
 * The single decision function the UI uses, mirroring `entitlements::require` in Rust.
 * Returning `true` when nothing is known keeps the UI from hiding features the backend
 * would still allow.
 */
export function granted(view: Entitlements | null, key: string): boolean {
  if (!view) return true;
  if (!view.enforced) return true;
  if (view.grace_expired) {
    return matchesAny(view.fallback_features.length ? view.fallback_features : view.free_keys, key);
  }
  return matchesAny(view.features, key);
}

export function cachedEntitlements(): Entitlements | null {
  return cached;
}

/** Read the cache without touching the network. */
export async function loadEntitlements(): Promise<Entitlements | null> {
  if (!isTauri()) return null;
  try {
    const view = await invoke<Entitlements>('entitlements_get');
    publish(view);
    return view;
  } catch {
    return cached;
  }
}

/**
 * Ask the server. Concurrent callers share one request; failures leave the last snapshot in
 * place so a flaky network never locks the user out.
 */
export async function refreshEntitlements(): Promise<Entitlements | null> {
  if (!isTauri()) return null;
  if (inflight) return inflight;
  inflight = invoke<Entitlements>('entitlements_refresh')
    .then(view => {
      publish(view);
      return view;
    })
    .catch(() => cached)
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

export async function listCapabilities(): Promise<Capability[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<Capability[]>('capabilities_list');
  } catch {
    return [];
  }
}

function useEntitlementsValue(): [Entitlements | null, boolean] {
  const [view, setView] = useState<Entitlements | null>(cached);
  const [loading, setLoading] = useState(cached === null && isTauri());

  useEffect(() => {
    let active = true;
    listeners.add(setView);
    if (!cached) {
      setLoading(true);
      void loadEntitlements().finally(() => {
        if (active) setLoading(false);
      });
    }
    return () => {
      active = false;
      listeners.delete(setView);
    };
  }, []);

  return [view, loading];
}

/** Start a background refresh once per app session: login, focus and a 6h timer. */
export function useEntitlements(): { view: Entitlements | null; loading: boolean; refresh: () => Promise<void> } {
  const [view, loading] = useEntitlementsValue();

  useEffect(() => {
    if (!isTauri()) return;
    void refreshEntitlements();
    const onFocus = () => void refreshEntitlements();
    window.addEventListener('focus', onFocus);
    const timer = window.setInterval(() => void refreshEntitlements(), 6 * 60 * 60 * 1000);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.clearInterval(timer);
    };
  }, []);

  const refresh = useCallback(async () => {
    await refreshEntitlements();
  }, []);

  return { view, loading, refresh };
}

/**
 * Internal-only surfaces — the agent access kit, local summaries and their 总结记录 — share
 * one gate: the server's `internal` flag. A missing or unsupported snapshot means "no".
 *
 * Reads the cached snapshot only, so callers that mount often never add network traffic.
 */
export function useInternalAccount(): { internal: boolean; loading: boolean; view: Entitlements | null } {
  const [view, loading] = useEntitlementsValue();
  return { internal: view?.internal === true, loading, view };
}

/**
 * Whether one capability is available right now. UI use only — the Rust command guard is the
 * actual boundary, so hiding a button never stands in for enforcement.
 */
export function useCapability(key: string): { allowed: boolean; loading: boolean; view: Entitlements | null } {
  const [view, loading] = useEntitlementsValue();
  return { allowed: granted(view, key), loading, view };
}

/** Numeric ceilings and quotas, with `undefined` when the plan does not define one. */
export function limitOf(view: Entitlements | null, key: string): number | undefined {
  return view?.limits?.[key];
}

export function quotaOf(view: Entitlements | null, key: string): number | undefined {
  return view?.quotas?.[key];
}

export function usageOf(view: Entitlements | null, key: string): number | undefined {
  return view?.usage?.[key];
}

/** Short human summary for settings, e.g. `Pro · 有效期至 2026-12-06`. */
export function planLabel(view: Entitlements | null): string {
  if (!view || !view.enforced) return t("未启用订阅控制");
  if (!view.plan) return t("免费版");
  const ended = view.plan.current_period_end
    ? tf(" · 有效期至 {0}", view.plan.current_period_end.slice(0, 10))
    : '';
  return tf("{0}{1}", view.plan.name || view.plan.slug || '当前套餐', ended);
}
