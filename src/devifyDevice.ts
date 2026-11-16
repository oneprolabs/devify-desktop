import { invoke } from '@tauri-apps/api/core'

let pending: Promise<unknown> | null = null

/**
 * Register this machine with Devify, deduplicating simultaneous requests.
 *
 * Both the shell startup path and the account settings page ask for a device
 * binding. Sharing one promise keeps a first launch from creating two device
 * rows when both run before the server returns the new device id.
 */
export function registerDevifyDevice() {
  if (!pending) {
    pending = invoke('devify_device_register').catch(() => null).finally(() => { pending = null })
  }
  return pending
}
