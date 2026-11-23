import { t, tf } from '../i18n/locale';
import './loading-skeleton.css';

type LoadingKind = 'list' | 'messages' | 'detail';

export function LoadingSkeleton({ label, kind = 'list' }: { label: string; kind?: LoadingKind }) {
  return <div className={`loading-skeleton loading-skeleton-${kind}`} role="status">
    <div className="loading-skeleton-label"><span className="loading-skeleton-spinner" aria-hidden="true"/>{t(label)}</div>
  </div>;
}

export function LoadingPage({ title }: { title: string }) {
  return <div className="loading-page">
    <aside className="loading-page-sidebar"><h1>{title}</h1><LoadingSkeleton label={tf("正在加载{0}", title)} /></aside>
    <main className="loading-page-main"><header>{title} <span>/</span> {t("全部")}</header><LoadingSkeleton label={tf("正在加载{0}", title)} kind="detail" /></main>
  </div>;
}
