import { t, tf, ctf } from './locale';
import './loading-skeleton.css';

type LoadingKind = 'list' | 'messages' | 'detail';

export function LoadingSkeleton({ label, kind = 'list' }: { label: string; kind?: LoadingKind }) {
  return <div className={`loading-skeleton loading-skeleton-${kind}`} role="status" aria-live="polite" aria-busy="true">
    <div className="loading-skeleton-label"><span className="loading-skeleton-spinner" aria-hidden="true"/>{t(label)}</div>
    <div className="loading-skeleton-items" aria-hidden="true">
      {[0, 1, 2, 3].map(item => <div className={`loading-skeleton-row row-${item}`} key={item}>
        {kind !== 'detail' && <span className="loading-skeleton-avatar shimmer"/>}
        <span className="loading-skeleton-lines"><i className="shimmer"/><i className="shimmer"/></span>
      </div>)}
    </div>
  </div>;
}

export function LoadingPage({ title }: { title: string }) {
  return <div className="loading-page">
    <aside className="loading-page-sidebar"><h1>{title}</h1><div className="loading-page-search"/><LoadingSkeleton label={tf("正在加载{0}", title)} /></aside>
    <main className="loading-page-main"><header>{title} <span>/</span> {t("全部")}</header><LoadingSkeleton label={tf("正在加载{0}", title)} kind="detail" /></main>
  </div>;
}
