import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

// Browser modules use bundler-style extensionless imports and TSX. Keep the
// production import graph intact while allowing Node to run the shared tests.
export function resolve(specifier, context, nextResolve) {
  if (specifier.startsWith('.') && context.parentURL?.startsWith('file:')) {
    const target = new URL(specifier, context.parentURL);
    if (!fs.existsSync(target)) {
      for (const extension of ['.ts', '.tsx', '.js']) {
        const candidate = new URL(target.href + extension);
        if (fs.existsSync(candidate)) return { url: candidate.href, shortCircuit: true };
      }
    }
  }
  return nextResolve(specifier, context);
}

export function load(url, context, nextLoad) {
  if (url.startsWith('file:') && url.endsWith('.css')) {
    return { format: 'module', shortCircuit: true, source: '' };
  }
  if (url.startsWith('file:') && /\.tsx?$/.test(url)) {
    const source = fs.readFileSync(fileURLToPath(url), 'utf8');
    return { format: 'module', shortCircuit: true, source: ts.transpileModule(source, { compilerOptions: {
      target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX,
    }, fileName: fileURLToPath(url) }).outputText };
  }
  return nextLoad(url, context);
}
