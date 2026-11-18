import { t, tf, ctf } from './locale';
import type { ReactNode } from 'react';

/** One conversation included in a saved summary. */
export type HistorySession = {
  username: string;
  display: string;
  is_group: boolean;
  messages: number;
};

/** Rule-based digest stored alongside local summaries. */
export type HistoryInsights = {
  focus: { category: string; session: string; day: string; text: string }[];
  timeline: { day: string; messages: number; sessions: string[] }[];
  related: { username: string; display: string; hits: number; truncated: boolean }[];
  terms: string[];
};

export type SummaryRecord = {
  id: string;
  created_at: string;
  kind: 'ai' | 'local' | 'daily';
  account_id: string;
  range_label: string;
  since: number | null;
  until: number | null;
  sessions: HistorySession[];
  messages: number;
  characters: number;
  truncated: boolean;
  agent_name: string;
  model: string;
  elapsed_ms: number;
  output: string | null;
  insights: HistoryInsights | null;
  note: string | null;
};

export type NewSummaryRecord = Omit<SummaryRecord, 'id' | 'created_at'>;

export const HISTORY_LIMIT = 50;

export const HISTORY_KIND_LABEL: Record<string, string> = {
  ai: 'AI 总结',
  local: '本地整理',
  daily: '自动日报',
};

/* ── Markdown ────────────────────────────────────────────────────────────────
   Agent output is untrusted text, so it is parsed into React elements instead
   of raw HTML: no dangerouslySetInnerHTML anywhere. Covers the subset the
   agents actually emit — headings, emphasis, inline/fenced code, links,
   bullets, ordered and nested lists, quotes, tables and rules. */

/** Only web-safe schemes become anchors; anything else stays plain text. */
function isSafeHref(href: string): boolean {
  return /^(https?:\/\/|mailto:)/i.test(href.trim());
}

function linkNode(href: string, label: ReactNode, key: string): ReactNode {
  const url = href.trim();
  if (!isSafeHref(url)) return <span key={key}>{label}</span>;
  return (
    <a key={key} href={url} target="_blank" rel="noreferrer noopener">
      {label}
    </a>
  );
}

/** Bare URLs become links too, but trailing punctuation is not part of the URL. */
function renderText(chunk: string, prefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const urlPattern = /https?:\/\/[^\s<>()"'`，。；：！？、）】]+/g;
  let cursor = 0;
  let index = 0;
  for (let match = urlPattern.exec(chunk); match; match = urlPattern.exec(chunk)) {
    if (match.index > cursor) nodes.push(chunk.slice(cursor, match.index));
    const url = match[0].replace(/[.,;:!?]+$/, '');
    nodes.push(linkNode(url, url, `${prefix}-url${index++}`));
    cursor = match.index + match[0].length;
  }
  if (cursor < chunk.length) nodes.push(chunk.slice(cursor));
  return nodes;
}

/** A fresh /g regex per call: nested emphasis re-enters this function, and a shared
 *  lastIndex would rewind the caller's scan and loop forever. */
const INLINE_PATTERN =
  /(`[^`\n]+`|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\n]+\*|_[^_\n]+_|!\[[^\]\n]*\]\([^)\s]*\)|\[[^\]\n]+\]\([^)\s]+\))/;

function renderInline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = new RegExp(INLINE_PATTERN.source, 'g');
  let cursor = 0;
  let index = 0;
  const pushText = (chunk: string) => {
    if (chunk) nodes.push(...renderText(chunk, `seg${index++}`));
  };
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    pushText(text.slice(cursor, match.index));
    cursor = match.index + match[0].length;
    const token = match[0];
    const key = `in${index++}`;
    if (token.startsWith('`')) {
      nodes.push(<code key={key}>{token.slice(1, -1)}</code>);
    } else if (token.startsWith('**') || token.startsWith('__')) {
      nodes.push(<strong key={key}>{renderInline(token.slice(2, -2))}</strong>);
    } else if (token.startsWith('~~')) {
      nodes.push(<del key={key}>{renderInline(token.slice(2, -2))}</del>);
    } else if (token.startsWith('*') || token.startsWith('_')) {
      nodes.push(<em key={key}>{renderInline(token.slice(1, -1))}</em>);
    } else if (token.startsWith('![')) {
      // Remote images would leak a request, so the alt text is shown instead.
      const alt = token.slice(2, token.indexOf(']'));
      nodes.push(<span key={key} className="md-image">{t("[图片")}{alt ? `：${alt}` : ''}]</span>);
    } else {
      const link = token.match(/^\[([^\]]*)\]\(([^)]*)\)$/);
      nodes.push(link ? linkNode(link[2], renderInline(link[1]), key) : token);
    }
  }
  pushText(text.slice(cursor));
  return nodes;
}

type ListEntry = { indent: number; ordered: boolean; start: number; text: string };

function renderListBlock(entries: ListEntry[], keyBase: string): ReactNode {
  const build = (start: number, indent: number): [ReactNode, number] => {
    const ordered = entries[start].ordered;
    const items: ReactNode[] = [];
    let cursor = start;
    let index = 0;
    while (cursor < entries.length && entries[cursor].indent >= indent) {
      if (entries[cursor].indent > indent) break;
      const entry = entries[cursor];
      cursor += 1;
      let nested: ReactNode = null;
      if (cursor < entries.length && entries[cursor].indent > indent) {
        const [node, next] = build(cursor, entries[cursor].indent);
        nested = node;
        cursor = next;
      }
      const task = entry.text.match(/^\[([ xX])\]\s+([\s\S]*)$/);
      items.push(
        <li key={`${keyBase}-${index++}`}>
          {task && <span className={`md-task ${task[1].toLowerCase() === 'x' ? 'done' : ''}`}>{task[1].toLowerCase() === 'x' ? '☑' : '☐'}</span>}
          {renderInline(task ? task[2] : entry.text)}
          {nested}
        </li>,
      );
    }
    const startNumber = ordered ? entries[start].start : 1;
    return ordered
      ? [<ol key={`${keyBase}-l${start}`} start={startNumber === 1 ? undefined : startNumber}>{items}</ol>, cursor]
      : [<ul key={`${keyBase}-l${start}`}>{items}</ul>, cursor];
  };
  const [node] = build(0, entries[0].indent);
  return node;
}

const HEADING_LINE = /^(#{1,6})\s+(.+?)\s*#*$/;
const RULE_LINE = /^([-*_])(?:\s*\1){2,}$/;
const QUOTE_LINE = /^>\s?/;
const BULLET_LINE = /^(\s*)[-*+]\s+(.*)$/;
const ORDERED_LINE = /^(\s*)(\d{1,3})[.)]\s+(.*)$/;
const FENCE_LINE = /^```+\s*([\w+#.-]*)\s*$/;

function splitRow(row: string): string[] {
  let value = row.trim();
  if (value.startsWith('|')) value = value.slice(1);
  if (value.endsWith('|')) value = value.slice(0, -1);
  return value.split('|').map(cell => cell.trim());
}

function isTableSeparator(row: string): boolean {
  if (!row.includes('-')) return false;
  return splitRow(row).every(cell => /^:?-{2,}:?$/.test(cell));
}

/** Chinese prose needs no space where a wrapped line meets the next one. */
function joinSoft(left: string, right: string): string {
  if (/[一-鿿，。；：！？、）】」』]$/.test(left) && /^[一-鿿（【「『]/.test(right)) return left + right;
  return `${left} ${right}`;
}

/** Markdown renderer for agent output: headings, lists, quotes, tables and code. */
export function renderAgentMarkdown(text: string): ReactNode[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ReactNode[] = [];
  let cursor = 0;

  while (cursor < lines.length) {
    const raw = lines[cursor];
    const line = raw.trimEnd();
    const trimmed = line.trim();

    if (!trimmed) {
      cursor += 1;
      continue;
    }

    const fence = trimmed.match(FENCE_LINE);
    if (fence) {
      const body: string[] = [];
      cursor += 1;
      while (cursor < lines.length && !/^```+\s*$/.test(lines[cursor].trim())) {
        body.push(lines[cursor]);
        cursor += 1;
      }
      cursor += 1;
      blocks.push(
        <pre key={`code-${cursor}`} className="md-code" data-lang={fence[1] || undefined}>
          <code>{body.join('\n')}</code>
        </pre>,
      );
      continue;
    }

    const heading = trimmed.match(HEADING_LINE);
    if (heading) {
      const level = heading[1].length;
      const Tag = (['h1', 'h2', 'h3', 'h4', 'h5', 'h6'] as const)[level - 1];
      blocks.push(<Tag key={`h-${cursor}`}>{renderInline(heading[2])}</Tag>);
      cursor += 1;
      continue;
    }

    if (RULE_LINE.test(trimmed)) {
      blocks.push(<hr key={`hr-${cursor}`} />);
      cursor += 1;
      continue;
    }

    if (trimmed.includes('|') && cursor + 1 < lines.length && isTableSeparator(lines[cursor + 1])) {
      const head = splitRow(trimmed);
      const align = splitRow(lines[cursor + 1]).map(cell => {
        const left = cell.startsWith(':');
        const right = cell.endsWith(':');
        return left && right ? 'center' : right ? 'right' : left ? 'left' : undefined;
      });
      cursor += 2;
      const rows: string[][] = [];
      while (cursor < lines.length && lines[cursor].trim() && lines[cursor].includes('|')) {
        rows.push(splitRow(lines[cursor]));
        cursor += 1;
      }
      blocks.push(
        <div key={`table-${cursor}`} className="md-table">
          <table>
            <thead>
              <tr>
                {head.map((cell, index) => (
                  <th key={`th-${index}`} style={align[index] ? { textAlign: align[index] } : undefined}>
                    {renderInline(cell)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row, rowIndex) => (
                <tr key={`tr-${rowIndex}`}>
                  {head.map((_, cellIndex) => (
                    <td key={`td-${cellIndex}`} style={align[cellIndex] ? { textAlign: align[cellIndex] } : undefined}>
                      {renderInline(row[cellIndex] ?? '')}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>,
      );
      continue;
    }

    if (QUOTE_LINE.test(trimmed)) {
      const quoted: string[] = [];
      while (cursor < lines.length && QUOTE_LINE.test(lines[cursor].trim())) {
        quoted.push(lines[cursor].trim().replace(QUOTE_LINE, ''));
        cursor += 1;
      }
      blocks.push(
        <blockquote key={`quote-${cursor}`}>{renderAgentMarkdown(quoted.join('\n'))}</blockquote>,
      );
      continue;
    }

    if (BULLET_LINE.test(line) || ORDERED_LINE.test(line)) {
      const entries: ListEntry[] = [];
      while (cursor < lines.length) {
        const current = lines[cursor];
        const bullet = current.match(BULLET_LINE);
        if (bullet) {
          entries.push({ indent: bullet[1].length, ordered: false, start: 1, text: bullet[2] });
          cursor += 1;
          continue;
        }
        const ordered = current.match(ORDERED_LINE);
        if (ordered) {
          entries.push({ indent: ordered[1].length, ordered: true, start: Number(ordered[2]), text: ordered[3] });
          cursor += 1;
          continue;
        }
        // A wrapped continuation line still belongs to the item above it.
        if (current.trim() && entries.length && /^\s{2,}/.test(current) && !RULE_LINE.test(current.trim())) {
          entries[entries.length - 1].text = joinSoft(entries[entries.length - 1].text, current.trim());
          cursor += 1;
          continue;
        }
        break;
      }
      blocks.push(renderListBlock(entries, `list-${cursor}`));
      continue;
    }

    const paragraph: string[] = [];
    while (cursor < lines.length) {
      const current = lines[cursor];
      const currentTrimmed = current.trim();
      if (
        !currentTrimmed ||
        FENCE_LINE.test(currentTrimmed) ||
        HEADING_LINE.test(currentTrimmed) ||
        RULE_LINE.test(currentTrimmed) ||
        QUOTE_LINE.test(currentTrimmed) ||
        BULLET_LINE.test(current) ||
        ORDERED_LINE.test(current) ||
        (currentTrimmed.includes('|') && cursor + 1 < lines.length && isTableSeparator(lines[cursor + 1]))
      ) {
        break;
      }
      paragraph.push(currentTrimmed);
      cursor += 1;
    }
    const prose = paragraph.reduce((acc, part) => (acc ? joinSoft(acc, part) : part), '');
    if (!paragraph.length) {
      // Unknown block marker: consume the line instead of spinning on it.
      blocks.push(<p key={`p-${cursor}`}>{renderInline(trimmed)}</p>);
      cursor += 1;
      continue;
    }
    blocks.push(<p key={`p-${cursor}`}>{renderInline(prose)}</p>);
  }

  return blocks;
}

/** Conversation names are the primary identity of a record, so they lead the title. */
export function summarySourceTitle(record: SummaryRecord): string {
  const names = record.sessions.map(session => session.display).filter(Boolean);
  if (!names.length) return record.range_label || t("未标记来源会话");
  if (names.length <= 2) return names.join('、');
  return tf("{0} 等 {1} 个会话", names.slice(0, 2).join('、'), names.length);
}

/**
 * Full record body, shared by the sidebar history page and any other reader.
 * `onDelete` / `onClose` are optional so the same component can serve an inline reader.
 */
export function SummaryDetail({
  record,
  onDelete,
  onClose,
}: {
  record: SummaryRecord;
  onDelete?: () => void;
  onClose?: () => void;
}) {
  const kindLabel = HISTORY_KIND_LABEL[record.kind] ?? record.kind;
  return <article className="history-detail">
    <header className="history-detail-head">
      <div className="history-detail-title">
        <span className={`history-kind ${record.kind}`}>{kindLabel}</span>
        <h3>{summarySourceTitle(record)}</h3>
        <p className="history-detail-meta">
          {record.created_at}
          {record.range_label ? ` · ${record.range_label}` : ''}
          {record.account_id ? tf(" · 账号 {0}", record.account_id) : ''}
          {record.agent_name ? ` · ${record.agent_name}` : ''}
          {record.model ? ` · ${record.model}` : ''}
          {record.elapsed_ms ? tf(" · {0} 秒", (record.elapsed_ms / 1000).toFixed(1)) : ''}
        </p>
      </div>
      <div className="history-detail-actions">
        {onDelete && <button type="button" className="secondary" onClick={onDelete}>{t("删除")}</button>}
        {onClose && <button type="button" className="secondary" onClick={onClose} aria-label={t("关闭这条记录")}>{t("关闭")}</button>}
      </div>
    </header>

    <section className="history-sources">
      {record.sessions.length ? <ul>{record.sessions.map(session => <li key={session.username}>
        <span className="history-source-icon" aria-hidden="true">◌</span>
        <span className="history-source-info"><strong>{session.display}</strong><small>{session.is_group ? t("群聊") : t("单聊")}{session.messages > 0 ? tf(" · {0} 条消息", session.messages) : ''}</small></span>
      </li>)}</ul> : <p className="history-empty-line">{t("这条记录没有保存来源会话。")}</p>}
      <p className="history-stats">{t("共")} {record.messages} {t("条消息 · 约")} {record.characters} {t("字")}{record.truncated ? t(" · 生成时已截断") : ''}</p>
    </section>

    {record.note && <div className="config warn">{t(record.note)}</div>}

    {record.output && (
      <div className="history-output wxv-ai-body">{renderAgentMarkdown(record.output)}</div>
    )}

    {record.insights && (
      <section className="history-insights">
        <div className="history-focus-heading"><h4>{t("项目重点")}</h4><span>{t("共")} {record.insights.focus.length} {t("条")}</span></div>
        <p className="history-insight-hint">{t("根据消息内容自动提取，结果仅供参考")}</p>
        {record.insights.focus.length ? (
          record.insights.focus.slice(0, 30).map((item, index) => (
            <article key={`focus-${index}`} className="history-focus">
              <span className="history-focus-marker" aria-hidden="true"/>
              <div>
                <small><span className={`wxv-insight-tag ${item.category}`}>{item.category}</span> {item.session} · {item.day}</small>
                <p>{item.text}</p>
              </div>
            </article>
          ))
        ) : (
          <p className="history-empty-line">{t("当时没有识别到待办、决定、风险、问题或数据线索。")}</p>
        )}
        <div className="history-related-grid"><section><h4>{t("时间线")}</h4>
        {record.insights.timeline.length ? (
          record.insights.timeline.map(day => (
            <div key={`day-${day.day}`} className="history-timeline-row">
              <strong>{day.day}</strong>
              <span>{day.messages} {t("条消息")}{day.sessions.length ? ` · ${day.sessions.join('、')}` : ''}</span>
            </div>
          ))
        ) : (
          <p className="history-empty-line">{t("没有可展示的时间线。")}</p>
        )}
        </section><section><h4>{t("相关会话")}</h4>
        {record.insights.related.length ? (
          record.insights.related.map(item => (
            <div key={`related-${item.username}`} className="history-timeline-row">
              <strong>{item.display}</strong>
              <span>{item.hits}{item.truncated ? '+' : ''} {t("条相关命中")}</span>
            </div>
          ))
        ) : (
          <p className="history-empty-line">{t("当时没有发现其他相关会话。")}</p>
        )}</section></div>
        {record.insights.terms.length > 0 && <p className="history-terms">{t("探索词：")}{record.insights.terms.join('、')}</p>}
      </section>
    )}
  </article>;
}
