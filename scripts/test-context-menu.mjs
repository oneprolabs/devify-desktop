import assert from 'node:assert/strict'
import { suppressDesktopContextMenu } from '../src/app/contextMenu.ts'
const target = editable => ({ closest: () => editable ? {} : null })
let prevented = false
suppressDesktopContextMenu({ target: target(false), preventDefault: () => { prevented = true } }, true)
assert.equal(prevented, true)
prevented = false
suppressDesktopContextMenu({ target: target(true), preventDefault: () => { prevented = true } }, true)
assert.equal(prevented, false)
suppressDesktopContextMenu({ target: target(false), preventDefault: () => { prevented = true } }, false)
assert.equal(prevented, false)
console.log('context menu checks passed')
