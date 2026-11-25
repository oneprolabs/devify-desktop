import { useEffect, useRef, useState } from 'react';
import { t } from '../../shared/i18n/locale';

const examples = [
  '通过 AIMyChats 读取「产品讨论群」昨天的聊天，整理结论、事情经过和待办。',
  '通过 AIMyChats 梳理「项目进度群」最近 7 天的进展，保留原文依据。',
  '通过 AIMyChats 列出我的未完成待办，打开第一条详情并保留证据引用。',
  '通过 AIMyChats 打开已经生成的上周周报，并查看「合同跟进」主题的已有报告，不要重新生成。',
];
export function AgentUsage() {
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const request = useRef(0);
  useEffect(() => () => { request.current += 1; }, []);
  const copy = async (example: string) => {
    const current = ++request.current; setNotice(''); setError('');
    try { await navigator.clipboard.writeText(t(example)); if (request.current === current) setNotice(t('示例已复制，可粘贴到智能体的新会话中。')); }
    catch { if (request.current === current) setError(t('复制失败，请重试或手动复制示例文字。')); }
  };
  return <div className="integration-usage" aria-labelledby="integration-usage-title">
    <h4 id="integration-usage-title">{t('怎么用')}</h4>
    <ol className="integration-usage-steps">
      <li><strong>{t('安装 Skills')}</strong><p>{t('选择上方常用智能体，或复制通用安装提示词。')}</p></li>
      <li><strong>{t('新建会话')}</strong><p>{t('在你的智能体中打开一个新会话，让技能生效。')}</p></li>
      <li><strong>{t('说明你要读什么')}</strong><p>{t('告诉它要读的聊天、待办或已有报告，以及时间范围。')}</p></li>
    </ol>
    <div className="integration-usage-examples">{examples.map(example => <div key={example}><p>{t(example)}</p>
      <button type="button" className="integration-text" aria-label={t('复制示例')} title={t('复制示例')} onClick={() => void copy(example)}><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><rect x="7" y="7" width="10" height="11" rx="2"/><path d="M13 7V2H2v11h5"/></svg>{t('复制')}</button></div>)}</div>
    {notice && <p className="integration-success" role="status">{notice}</p>}{error && <p className="integration-error" role="alert">{error}</p>}
  </div>;
}
