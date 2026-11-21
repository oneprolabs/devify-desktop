import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const documents = await Promise.all([
  ['ASYNC_TASK_DESIGN.md', '设计方案'],
  ['ASYNC_TASK_CURRENT_STATE.md', '历史附录'],
].map(async ([file, kind]) => ({ file, kind, source: await fs.readFile(path.join(root, file), 'utf8') })));
const source = documents.map(document => document.source).join('\n');
const hash = value => createHash('sha256').update(value).digest('hex').slice(0, 12);
const diagrams = documents.flatMap(document => [...document.source.matchAll(/<!-- flow:([\w-]+) -->\n([\s\S]*?)<!-- \/flow -->/g)].map(([, id, body]) => {
  const title = body.match(/^## (.+)$/m)?.[1].replace(/^(?:\d+\.|A\.\d+)\s*/, '');
  const summary = body.match(/^(?:目标|现状)：(.+)$/m)?.[1];
  const reviewedAt = body.match(/^审核：已审核（用户确认，(\d{4}-\d{2}-\d{2})）/m)?.[1] ?? null;
  const diagram = body.match(/```mermaid\n([\s\S]*?)```/)?.[1].trim();
  if (!title || !summary || !diagram || !diagram.includes('Note over')) throw new Error(`Incomplete annotated sequence: ${document.file} ${id}`);
  const entities = [...diagram.matchAll(/^\s*(?:actor|participant) (\w+) as (.+)$/gm)].map(([, key, label]) => `${key}:${label}`);
  const expected = ['U:用户', 'C:客户端', 'D:客户端数据库', 'F:客户端配置文件', 'S:服务端'];
  const group = diagram.match(/^\s*box [^\n]*客户端组\n([\s\S]*?)^\s*end\s*$/m)?.[1];
  if (JSON.stringify(entities) !== JSON.stringify(expected) || !group || !['C', 'D', 'F'].every(key => group.includes(`participant ${key} as `))) throw new Error(`Inconsistent entity model: ${document.file} ${id}`);
  for (const [, from, to] of diagram.matchAll(/^\s*(\w+)-{1,2}>>?(\w+):/gm)) {
    if (![from, to].every(key => ['U', 'C', 'D', 'F', 'S'].includes(key))) throw new Error(`Undeclared entity in ${id}: ${from} → ${to}`);
  }
  return { id, title, summary, reviewedAt, source: diagram, sourceHash: hash(diagram), kind: document.kind, document: document.file };
}));
if (!diagrams.length || new Set(diagrams.map(item => item.id)).size !== diagrams.length || [...source.matchAll(/```mermaid\n/g)].length !== diagrams.length || source.includes('<!-- interfaces:')) throw new Error('Every diagram needs one unique flow block; communication details belong inside its sequence, not a separate interface table.');
const json = value => JSON.stringify(value).replaceAll('<', '\\u003c');
const mode = process.argv[2];
if (mode === '--prepare') {
  const directory = path.resolve(process.argv[3]);
  const library = path.resolve(process.argv[4]);
  await fs.mkdir(directory, { recursive: true });
  await fs.copyFile(library, path.join(directory, 'mermaid.js'));
  const bootstrap = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><title>Render lifecycle diagrams</title>
<script src="mermaid.js"></script><body><p id="status">Rendering…</p><script>
const diagrams=${json(diagrams)};
mermaid.initialize({startOnLoad:false,securityLevel:'strict',theme:'base',fontFamily:'PingFang SC, Microsoft YaHei, sans-serif',themeVariables:{fontSize:'15px',primaryColor:'#ffffff',primaryTextColor:'#252522',primaryBorderColor:'#d9d9d4',lineColor:'#8d8d85',secondaryColor:'#f4f4f1',tertiaryColor:'#faf7fa',noteBkgColor:'#fbf5fa',noteTextColor:'#583653',noteBorderColor:'#dcc9d9',actorBkgColor:'#f5f5f2',actorBkg:'#f5f5f2',actorBorder:'#cecec7',actorTextColor:'#292925',signalColor:'#62625d',signalTextColor:'#272724',labelBoxBkgColor:'#f2f2ee',labelBoxBorderColor:'#deded7',labelTextColor:'#41413b',loopTextColor:'#41413b',sequenceNumberColor:'#ffffff'},sequence:{useMaxWidth:false,wrap:true,actorMargin:45,boxMargin:12,diagramMarginX:18,diagramMarginY:18,messageMargin:30,noteMargin:12,width:155,showSequenceNumbers:true}});
(async()=>{window.rendered=[];for(const item of diagrams){const {svg}=await mermaid.render(item.id,item.source);window.rendered.push({id:item.id,sourceHash:item.sourceHash,svg});}document.getElementById('status').textContent='Rendered '+window.rendered.length;window.renderDone=true;})().catch(error=>{window.renderError=String(error);document.getElementById('status').textContent=String(error);});
</script></body></html>`;
  await fs.writeFile(path.join(directory, 'index.html'), bootstrap);
  console.log(`Renderer prepared: ${directory}`);
} else if (mode === '--svgs') {
  const svgs = JSON.parse(await fs.readFile(process.argv[3], 'utf8'));
  const rendered = new Map(svgs.map(item => [item.id, item]));
  if (svgs.length !== diagrams.length || rendered.size !== diagrams.length || diagrams.some(item => rendered.get(item.id)?.sourceHash !== item.sourceHash || !rendered.get(item.id)?.svg?.includes('<svg'))) throw new Error('Rendered SVGs must match each current diagram ID and source hash. Render again after diagram edits.');
  const template = await fs.readFile(path.join(root, 'scripts/async-task-preview.template.html'), 'utf8');
  const html = template.replace('/*__FLOW_DATA__*/', json(diagrams.map(item => ({ ...item, svg: rendered.get(item.id).svg.replace(/-(<\/(?:text|tspan)>)/g, '$1') }))))
    .replaceAll('__SOURCE_HASH__', createHash('sha256').update(source).digest('hex').slice(0, 12));
  const output = path.join(root, 'ASYNC_TASK_PREVIEW.html');
  await fs.writeFile(output, html);
  console.log(`Built ${output} (${Buffer.byteLength(html)} bytes); no runtime network dependencies.`);
} else {
  throw new Error('Usage: --prepare <render-directory> <mermaid-11.12.1.min.js>, then --svgs <rendered-svgs.json>');
}
