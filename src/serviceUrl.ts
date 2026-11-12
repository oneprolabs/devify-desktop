const OFFICIAL_API_BASE = 'https://app.aimychats.com';
const OFFICIAL_DISPLAY_BASE = 'https://aimychats.com/';
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

export function displayBaseUrl(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return '';
  return OFFICIAL_HOSTS.has(parsedHost(trimmed.replace(/\/+$/, ''))) ? OFFICIAL_DISPLAY_BASE : trimmed;
}

export function sameServiceUrl(left: string, right: string): boolean {
  const a = apiBaseUrl(left);
  const b = apiBaseUrl(right);
  return !!a && a === b;
}
