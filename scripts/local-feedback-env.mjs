#!/usr/bin/env node
/**
 * Local feedback test environment.
 *
 * The desktop app talks to one base URL for everything, so a feedback-only stub would
 * log the user out. This server answers the feedback endpoints itself and proxies every
 * other request to the real service, which keeps login, spaces and entitlements working
 * while `POST /api/v1/feedbacks` succeeds against a local store.
 *
 *   node scripts/local-feedback-env.mjs
 *
 * Environment:
 *   FEEDBACK_ENV_PORT      listen port (default 8010)
 *   FEEDBACK_ENV_UPSTREAM  real service to proxy (default https://app.aimychats.com)
 *   FEEDBACK_ENV_STORE     report store (default <app data>/local-feedback)
 *
 * Then point the app's 账户 › 服务地址 at http://127.0.0.1:8010. For a PATCH-able
 * support reply, call `/api/v1/admin/feedbacks/<id>` with `{status, reply, fix_version}`.
 */
import { createServer, request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, renameSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const PORT = Number(process.env.FEEDBACK_ENV_PORT || 8010);
const UPSTREAM = new URL(process.env.FEEDBACK_ENV_UPSTREAM || 'https://app.aimychats.com');
const STORE = process.env.FEEDBACK_ENV_STORE
  || join(homedir(), 'Library', 'Application Support', 'local.wechat.devifybridge', 'local-feedback');
const REPORTS = join(STORE, 'reports.json');
const ATTACHMENTS = join(STORE, 'attachments');
const CATEGORIES = new Set(['wechat', 'backup', 'upload', 'login', 'interface', 'other']);
const PAGES = new Set(['chat', 'events', 'topics', 'reports', 'ask', 'tasks', 'settings', 'login', 'app']);
const MAX_BODY = 12 * 1024 * 1024;
const PAGE_SIZE = 20;

mkdirSync(ATTACHMENTS, { recursive: true });

function load() {
  if (!existsSync(REPORTS)) return [];
  try {
    const parsed = JSON.parse(readFileSync(REPORTS, 'utf8'));
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function store(reports) {
  const temporary = `${REPORTS}.tmp`;
  writeFileSync(temporary, JSON.stringify(reports, null, 2));
  renameSync(temporary, REPORTS);
}

const reports = load();

function view(report) {
  return {
    id: report.id,
    category: report.category,
    description: report.description,
    status: report.status,
    reply: report.reply,
    fix_version: report.fix_version,
    created_at: report.created_at,
  };
}

/** Wrap like the service's renderer: {code, message, data}. */
function send(response, status, data, message = '') {
  const body = JSON.stringify({ code: status >= 400 ? status : 0, message, data });
  response.writeHead(status, { 'content-type': 'application/json; charset=utf-8', 'content-length': Buffer.byteLength(body) });
  response.end(body);
}

function userId(request) {
  const header = request.headers.authorization || '';
  const token = header.replace(/^bearer\s+/i, '');
  const payload = token.split('.')[1];
  if (!payload) return null;
  try {
    const claims = JSON.parse(Buffer.from(payload, 'base64url').toString('utf8'));
    return typeof claims.user_id === 'number' && claims.user_id > 0 ? claims.user_id : null;
  } catch {
    return null;
  }
}

function readBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    request.on('data', chunk => {
      size += chunk.length;
      if (size > MAX_BODY) {
        reject(new Error('too large'));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on('end', () => resolve(Buffer.concat(chunks)));
    request.on('error', reject);
  });
}

/** Split a multipart body into {name, filename, type, data} parts. */
function multipart(body, contentType) {
  const boundary = /boundary=(?:"([^"]+)"|([^;]+))/i.exec(contentType || '');
  if (!boundary) return [];
  const marker = Buffer.from(`--${boundary[1] || boundary[2]}`);
  const parts = [];
  let cursor = body.indexOf(marker);
  while (cursor >= 0) {
    const start = cursor + marker.length;
    if (body.subarray(start, start + 2).toString() === '--') break;
    const next = body.indexOf(marker, start);
    if (next < 0) break;
    const chunk = body.subarray(start + 2, next - 2);
    const split = chunk.indexOf('\r\n\r\n');
    if (split > 0) {
      const head = chunk.subarray(0, split).toString('utf8');
      const name = /name="([^"]*)"/i.exec(head)?.[1] || '';
      const filename = /filename="([^"]*)"/i.exec(head)?.[1];
      const type = /content-type:\s*([^\r\n]+)/i.exec(head)?.[1]?.trim() || 'application/octet-stream';
      parts.push({ name, filename, type, data: chunk.subarray(split + 4) });
    }
    cursor = next;
  }
  return parts;
}

function fingerprint(payload, screenshots) {
  return JSON.stringify({
    category: payload.category,
    description: payload.description,
    context: payload.context,
    diagnostics: payload.diagnostics ?? null,
    screenshots: screenshots.map(part => part.data.toString('base64')),
  });
}

function submit(request, response, body) {
  const parts = multipart(body, request.headers['content-type']);
  const payloadPart = parts.find(part => part.name === 'payload');
  if (!payloadPart) return send(response, 400, null, '缺少反馈内容。');
  let payload;
  try {
    payload = JSON.parse(payloadPart.data.toString('utf8'));
  } catch {
    return send(response, 400, null, '反馈数据格式无效。');
  }
  if (!CATEGORIES.has(payload.category)) return send(response, 400, null, '问题类型无效。');
  if (typeof payload.description !== 'string' || !payload.description.trim() || payload.description.length > 5000) {
    return send(response, 400, null, '请描述遇到的问题（最多 5000 字）。');
  }
  const context = payload.context || {};
  if (typeof context.app_version !== 'string' || !['macos', 'windows', 'linux'].includes(context.platform)) {
    return send(response, 400, null, '运行环境无效。');
  }
  if (context.page && !PAGES.has(context.page)) return send(response, 400, null, '页面标识无效。');
  const screenshots = parts.filter(part => part.name === 'screenshots');
  if (screenshots.length > 3) return send(response, 400, null, '最多添加 3 张截图。');
  for (const shot of screenshots) {
    const png = shot.data.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    const jpeg = shot.data.subarray(0, 3).equals(Buffer.from([0xff, 0xd8, 0xff]));
    if (!png && !jpeg) return send(response, 400, null, '仅支持 PNG/JPEG 截图。');
  }
  const digest = fingerprint(payload, screenshots);
  const existing = reports.find(report => report.client_id === payload.client_id);
  if (existing) {
    // Same content is a safe retry; changed content must not overwrite a delivered report.
    if (existing.fingerprint === digest) return send(response, 200, view(existing));
    return send(response, 409, null, '提交编号对应的内容已改变，请查看我的反馈或新建反馈。');
  }
  const id = randomUUID();
  const attachments = screenshots.map(shot => {
    const attachment = randomUUID();
    const extension = shot.type.includes('jpeg') ? 'jpg' : 'png';
    writeFileSync(join(ATTACHMENTS, `${attachment}.${extension}`), shot.data);
    return { id: attachment, mime_type: shot.type, size: shot.data.length };
  });
  const report = {
    id,
    user_id: userId(request),
    client_id: payload.client_id,
    category: payload.category,
    description: payload.description,
    context,
    diagnostics: payload.diagnostics ?? null,
    attachments,
    fingerprint: digest,
    status: 'received',
    reply: '',
    fix_version: '',
    created_at: new Date().toISOString(),
  };
  reports.push(report);
  store(reports);
  console.log(`[feedback] new report ${id} (${report.category}) attachments=${attachments.length}`);
  return send(response, 201, view(report));
}

function list(request, response, url) {
  const owner = userId(request);
  const mine = reports.filter(report => report.user_id === owner).sort((a, b) => b.created_at.localeCompare(a.created_at));
  const page = Math.min(Math.max(Number(url.searchParams.get('page') || 1), 1), 10000);
  const start = (page - 1) * PAGE_SIZE;
  const next = start + PAGE_SIZE < mine.length
    ? `http://127.0.0.1:${PORT}/api/v1/feedbacks?page=${page + 1}`
    : null;
  return send(response, 200, {
    count: mine.length,
    next,
    previous: page > 1 ? `http://127.0.0.1:${PORT}/api/v1/feedbacks?page=${page - 1}` : null,
    results: mine.slice(start, start + PAGE_SIZE).map(view),
  });
}

function reply(response, reportId, body) {
  const report = reports.find(item => item.id === reportId);
  if (!report) return send(response, 404, null, '反馈不存在。');
  let update;
  try {
    update = JSON.parse(body.toString('utf8'));
  } catch {
    return send(response, 400, null, '更新内容无效。');
  }
  if (typeof update.status === 'string') report.status = update.status;
  if (typeof update.reply === 'string') report.reply = update.reply;
  if (typeof update.fix_version === 'string') report.fix_version = update.fix_version;
  store(reports);
  console.log(`[feedback] report ${reportId} -> ${report.status}`);
  return send(response, 200, view(report));
}

function proxy(request, response, body) {
  const target = new URL(request.url, UPSTREAM);
  const headers = { ...request.headers, host: target.host };
  delete headers['content-length'];
  if (body.length) headers['content-length'] = String(body.length);
  const transport = target.protocol === 'https:' ? httpsRequest : httpRequest;
  const upstream = transport({
    protocol: target.protocol,
    hostname: target.hostname,
    port: target.port || (target.protocol === 'https:' ? 443 : 80),
    path: `${target.pathname}${target.search}`,
    method: request.method,
    headers,
  }, answer => {
    response.writeHead(answer.statusCode || 502, answer.headers);
    answer.pipe(response);
  });
  upstream.on('error', error => {
    console.log(`[proxy] ${request.method} ${target.pathname} failed: ${error.message}`);
    if (!response.headersSent) send(response, 502, null, '上游服务不可用。');
    else response.end();
  });
  if (body.length) upstream.write(body);
  upstream.end();
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url, `http://127.0.0.1:${PORT}`);
  let body = Buffer.alloc(0);
  if (request.method === 'POST' || request.method === 'PATCH' || request.method === 'PUT') {
    try {
      body = await readBody(request);
    } catch {
      return send(response, 413, null, '请求内容过大。');
    }
  }
  if (url.pathname === '/health') return send(response, 200, { status: 'ok', reports: reports.length });
  const feedback = url.pathname === '/api/v1/feedbacks' || url.pathname.startsWith('/api/v1/feedbacks/');
  if (feedback) {
    if (!userId(request)) return send(response, 401, null, '登录已失效，请重新登录。');
    if (request.method === 'POST' && url.pathname === '/api/v1/feedbacks') return submit(request, response, body);
    if (request.method === 'GET' && url.pathname === '/api/v1/feedbacks') return list(request, response, url);
    return send(response, 405, null, '不支持的方法。');
  }
  const admin = /^\/api\/v1\/admin\/feedbacks\/([0-9a-f-]{36})$/.exec(url.pathname);
  if (admin && request.method === 'PATCH') return reply(response, admin[1], body);
  return proxy(request, response, body);
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`feedback test environment: http://127.0.0.1:${PORT}`);
  console.log(`  store    ${STORE} (${reports.length} reports)`);
  console.log(`  upstream ${UPSTREAM.origin} (everything outside /api/v1/feedbacks)`);
});
