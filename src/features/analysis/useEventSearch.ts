import { useEffect, useState } from 'react';

/** Capability-gated search: old servers keep local filtering, not ignored q requests. */
export function useEventSearch(identity: string, query: string, enabled: boolean) {
  const [capability, setCapability] = useState<{ identity: string; supported: boolean } | null>(null);
  const [settled, setSettled] = useState({ identity, keyword: '' });
  const supported = enabled && capability?.identity === identity && capability.supported;
  const target = supported ? query.trim() : '';
  const tooLong = supported && Array.from(target).length > 200;
  const keyword = supported && settled.identity === identity ? settled.keyword : '';
  const waiting = !tooLong && supported && target !== keyword;

  useEffect(() => {
    if (!waiting) return;
    const timer = setTimeout(() => setSettled({ identity, keyword: target }), 350);
    return () => clearTimeout(timer);
  }, [identity, target, waiting]);

  return {
    supported, keyword, waiting, tooLong,
    setSupported: (supported: boolean) => setCapability(current => current?.identity === identity && current.supported === supported ? current : { identity, supported }),
    reset: () => { setCapability(null); setSettled({ identity, keyword: '' }); },
  };
}
