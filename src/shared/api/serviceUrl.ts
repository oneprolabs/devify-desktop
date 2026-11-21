const OFFICIAL_API_BASE = 'https://app.aimychats.com';
const OFFICIAL_HOSTS = new Set(['aimychats.com', 'www.aimychats.com', 'app.aimychats.com']);

function parsedHost(value: string): string {
  try {
    return new URL(value.trim()).hostname.toLowerCase();
  } catch {
    return '';
  }
}

export function apiBaseUrl(value: string): string {
  const trimmed = value.trim().replace(/\/+$/, '');
  if (!trimmed) return '';
  return OFFICIAL_HOSTS.has(parsedHost(trimmed)) ? OFFICIAL_API_BASE : trimmed;
}

/**
 * The value shown in the address field and persisted in settings. It is exactly what the
 * user typed, only trimmed: mapping the official product hosts onto the API origin happens
 * while building requests, so saving `https://app.aimychats.com/` must not change it.
 */
export function storedBaseUrl(value: string): string {
  return value.trim();
}

export function sameServiceUrl(left: string, right: string): boolean {
  const a = apiBaseUrl(left);
  const b = apiBaseUrl(right);
  return !!a && a === b;
}
