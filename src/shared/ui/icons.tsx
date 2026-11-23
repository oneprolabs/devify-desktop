import type { ReactNode } from 'react';

/* One authored icon set for the whole app: 16px grid, 1.4px stroke, round caps.
   Font glyphs and emoji are never used as icons — they render at whatever weight
   and optical size the system font decides, which breaks the native look. */

const stroke = (children: ReactNode) => (
  <svg
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth={1.4}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    {children}
  </svg>
);

const filled = (children: ReactNode) => (
  <svg viewBox="0 0 16 16" fill="currentColor" stroke="none" aria-hidden="true" focusable="false">
    {children}
  </svg>
);

export const Icons = {
  /* navigation */
  chat: stroke(<>
    <path d="M2.9 4.4a1.9 1.9 0 0 1 1.9-1.9h6.4a1.9 1.9 0 0 1 1.9 1.9v4.4a1.9 1.9 0 0 1-1.9 1.9H7.3L4.1 13.3v-2.6h-.3a1.9 1.9 0 0 1-.9-1.9z" />
    <path d="M5.6 5.9h4.8M5.6 8h3" />
  </>),
  history: stroke(<>
    <path d="M4.1 2.5h4.5l3.3 3.3v7.7H4.1z" />
    <path d="M8.5 2.6v3.2h3.2" />
    <path d="M6.1 9.1h3.8M6.1 11.3h2.4" />
  </>),
  allow: stroke(<>
    <path d="M2.6 5.2 3.9 6.5 6.1 4.3" />
    <path d="M9.2 5.3h4.1" />
    <path d="M2.6 11.2 3.9 12.5 6.1 10.3" />
    <path d="M9.2 11.3h4.1" />
  </>),
  clock: stroke(<>
    <circle cx="8" cy="8.2" r="5.3" />
    <path d="M8 5.1v3.3l2.2 1.3" />
  </>),
  agent: stroke(<>
    <path d="M8 2.6 9.4 6.6 13.4 8 9.4 9.4 8 13.4 6.6 9.4 2.6 8 6.6 6.6z" />
  </>),
  send: stroke(<>
    <path d="M13.4 2.9 2.9 6.9l4 1.9 1.9 4z" />
    <path d="M13.4 2.9 6.9 8.8" />
  </>),
  key: stroke(<>
    <circle cx="5.4" cy="10.5" r="2.5" />
    <path d="M7.2 8.7 12.5 3.4" />
    <path d="M10.3 5.6l1.4 1.4M11.9 4l1.3 1.3" />
  </>),
  info: stroke(<>
    <circle cx="8" cy="8" r="5.3" />
    <path d="M8 7.2v4M8 5.1v.3" />
  </>),

  /* actions and status */
  check: stroke(<path d="M3.7 8.3 6.4 11 12.3 4.9" />),
  close: stroke(<path d="M4.4 4.4 11.6 11.6M11.6 4.4 4.4 11.6" />),
  chevronLeft: stroke(<path d="M9.7 3.9 5.6 8l4.1 4.1" />),
  networkAuto: stroke(<>
    <path d="M2.5 8h2a4 4 0 0 0 3-1.5A4 4 0 0 1 10.5 5h3m-2-2 2 2-2 2" />
    <path d="M4.5 8a4 4 0 0 1 3 1.5 4 4 0 0 0 3 1.5h3m-2-2 2 2-2 2" />
  </>),
  networkProxy: stroke(<>
    <rect x="5.6" y="5" width="4.8" height="6" rx="1" />
    <path d="M2 8h3.6m4.8 0H14" />
  </>),
  networkDirect: stroke(<>
    <circle cx="3.5" cy="8" r="1.5" />
    <path d="M5 8h6" />
    <circle cx="12.5" cy="8" r="1.5" />
  </>),
  refresh: stroke(<>
    <path d="M13.1 8A5.1 5.1 0 1 1 11 3.9" />
    <path d="M13.2 2.9v3.4H9.8" />
  </>),
  pause: stroke(<path d="M6.3 4.3v7.4M9.7 4.3v7.4" />),
  play: stroke(<path d="M6.2 4.2 11.4 8 6.2 11.8z" />),
  warning: stroke(<>
    <path d="M8 2.9 14 12.8H2z" />
    <path d="M8 6.4v2.9M8 11.2v.3" />
  </>),
  minus: stroke(<path d="M4.9 8h6.2" />),
  download: stroke(<>
    <path d="M8 2.9v7.1" />
    <path d="M5.3 7.4 8 10.1l2.7-2.7" />
    <path d="M3.1 12.8h9.8" />
  </>),  list: stroke(<>
    <path d="M3.1 4.7h1.1M6.4 4.7h6.5" />
    <path d="M3.1 8h1.1M6.4 8h6.5" />
    <path d="M3.1 11.3h1.1M6.4 11.3h4.4" />
  </>),

  /* the one solid mark: the running-sync indicator */
  dot: filled(<circle cx="8" cy="8" r="2.6" />),
};

export type IconName = keyof typeof Icons;

/* Left rail icons: the prototype draws these on a 24px grid with a 1.6 stroke, so the rail
   keeps its own set instead of scaling the 16px button icons up. Paths are copied verbatim
   from protype.html so the two stay identical. */
const rail = (children: ReactNode) => (
  <svg
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth={1.6}
    strokeLinejoin="round"
    strokeLinecap="round"
    aria-hidden="true"
    focusable="false"
  >
    {children}
  </svg>
);

export const RailIcons = {
  chat: rail(<path d="M4 5.5h16v10H9l-4 3.5v-3.5H4z" />),
  events: rail(<>
    <rect x="4.5" y="4.5" width="15" height="15" rx="2.5" />
    <path d="M8.5 12l2.5 2.5 4.5-5" />
  </>),
  topics: rail(<>
    <path d="M6 16.5V11a6 6 0 0112 0v5.5l1.5 1.5h-15z" />
    <path d="M10 20.5h4" />
  </>),
  reports: rail(<>
    <path d="M6.5 3.5h8l3 3v14h-11z" />
    <path d="M9.5 11h5M9.5 14.5h5" />
  </>),
  ask: rail(<>
    <path d="M12 3.8l1.9 5 5 1.9-5 1.9-1.9 5-1.9-5-5-1.9 5-1.9z" />
    <path d="M18.5 15.5l.8 2 2 .8-2 .8-.8 2-.8-2-2-.8 2-.8z" />
  </>),
  tasks: rail(<>
    <path d="M4 8l8-4 8 4-8 4z" />
    <path d="M4 12l8 4 8-4" />
    <path d="M4 16l8 4 8-4" />
  </>),
  settings: rail(<>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 3.5v2.5M12 18v2.5M3.5 12H6M18 12h2.5M6 6l1.8 1.8M16.2 16.2L18 18M6 18l1.8-1.8M16.2 7.8L18 6" />
  </>),
};
