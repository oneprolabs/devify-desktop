import { registerHooks } from 'node:module';
import { resolve, load } from './test-module-loader.mjs';
registerHooks({ resolve, load });
