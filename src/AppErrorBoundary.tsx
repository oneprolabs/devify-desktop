import { Component, type ReactNode } from 'react';
import { recordDiagnostic } from './desktopInvoke';
import { Feedback } from './Feedback';

export class AppErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch() { recordDiagnostic({ module: 'ui', operation: 'exception', code: 'RENDER_ERROR' }); }
  render() {
    if (!this.state.failed) return this.props.children;
    return <main className="feedback-crash"><h1>页面遇到问题</h1><p>可以保存诊断信息或提交反馈，帮助我们定位问题。</p><button type="button" onClick={() => window.location.reload()}>重新加载</button><Feedback page="app" /></main>;
  }
}
