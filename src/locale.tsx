import { useSyncExternalStore } from 'react';
import { en } from './enCatalog';

export type UiLanguage = '中文' | 'English';
type LanguageChoice = { ui?: string; summary?: string };

let language: UiLanguage = '中文';
let contentEnglish = false;
const listeners = new Set<() => void>();

function emit() {
  if (typeof document !== 'undefined') document.documentElement.lang = language === 'English' ? 'en' : 'zh-CN';
  listeners.forEach(listener => listener());
}

export function getUiLanguage(): UiLanguage {
  return language;
}

export function applyLanguage(value?: LanguageChoice) {
  const next: UiLanguage = value?.ui === 'English' ? 'English' : '中文';
  const summary = value?.summary ?? '跟随界面';
  const nextContent = summary === 'English' || (summary === '跟随界面' && next === 'English');
  if (next === language && nextContent === contentEnglish) return;
  language = next;
  contentEnglish = nextContent;
  emit();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useUiLanguage() {
  return useSyncExternalStore(subscribe, getUiLanguage, getUiLanguage);
}

function lookup(text: string, english: boolean) {
  if (!english || !text) return text;
  return en[text] ?? text;
}

/** Interface copy. The Chinese source stays in code; English is chosen at render. */
export function t(text: string): string {
  return lookup(text, language === 'English');
}

/** Summary and report sentences that follow 总结与报告语言. */
export function cf(text: string): string {
  return lookup(text, contentEnglish);
}

function fill(template: string, args: unknown[]) {
  return template.replace(/\{(\d+)\}/g, (_, index: string) => String(args[Number(index)] ?? ''));
}

export function tf(pattern: string, ...args: unknown[]): string {
  return fill(lookup(pattern, language === 'English'), args);
}

export function ctf(pattern: string, ...args: unknown[]): string {
  return fill(lookup(pattern, contentEnglish), args);
}

export function dateLocale() {
  return language === 'English' ? 'en' : 'zh-CN';
}

export function sortLocale() {
  return language === 'English' ? 'en' : 'zh-Hans-CN';
}
