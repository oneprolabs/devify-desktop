import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import ts from 'typescript';
import { en } from '../src/shared/i18n/enCatalog.ts';

const han = /\p{Script=Han}/u;
const textAttributes = new Set(['aria-label', 'aria-description', 'alt', 'busyLabel', 'description', 'emptyText', 'label', 'placeholder', 'title', 'tooltip']);
const missing = [];

function checkDirectory(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const filename = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      checkDirectory(filename);
    } else if (/\.tsx?$/.test(filename)) {
      const source = ts.createSourceFile(filename, fs.readFileSync(filename, 'utf8'), ts.ScriptTarget.Latest, true);
      const report = (node, message) => {
        const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
        missing.push(`${filename}:${line + 1} ${message}`);
      };
      const checkVisible = node => {
        if (ts.isStringLiteralLike(node) && han.test(node.text)) {
          report(node, node.text);
        } else if (ts.isTemplateExpression(node) && [node.head, ...node.templateSpans.map(span => span.literal)].some(part => han.test(part.text))) {
          report(node, node.getText(source));
        } else if (ts.isConditionalExpression(node)) {
          checkVisible(node.whenTrue);
          checkVisible(node.whenFalse);
        } else if (ts.isBinaryExpression(node) && [ts.SyntaxKind.AmpersandAmpersandToken, ts.SyntaxKind.BarBarToken, ts.SyntaxKind.QuestionQuestionToken].includes(node.operatorToken.kind)) {
          if (node.operatorToken.kind !== ts.SyntaxKind.AmpersandAmpersandToken) checkVisible(node.left);
          checkVisible(node.right);
        }
      };
      const visit = node => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && ['t', 'tf', 'cf', 'ctf'].includes(node.expression.text)) {
          const [message] = node.arguments;
          if (message && ts.isStringLiteralLike(message) && han.test(message.text) && !Object.hasOwn(en, message.text)) report(message, message.text);
        }
        if (ts.isJsxText(node) && han.test(node.text)) report(node, node.text.trim());
        if (ts.isJsxExpression(node) && node.expression && (ts.isJsxElement(node.parent) || ts.isJsxFragment(node.parent))) checkVisible(node.expression);
        if (ts.isJsxAttribute(node) && textAttributes.has(node.name.getText(source)) && node.initializer) {
          const value = ts.isJsxExpression(node.initializer) ? node.initializer.expression : node.initializer;
          if (value) checkVisible(value);
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
  }
}

checkDirectory('src');
const placeholders = text => [...new Set(text.match(/\{\d+\}/g) ?? [])].sort();
for (const [source, translated] of Object.entries(en)) {
  assert.deepEqual(placeholders(translated), placeholders(source), `Translation placeholders for ${source}`);
}
assert.deepEqual(missing, [], 'Client-authored copy must have an English translation or use a localized template.');
console.log('English copy coverage checks passed');
