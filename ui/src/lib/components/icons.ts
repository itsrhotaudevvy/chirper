/** Icon paths on a 24x24 grid, stroked (1.75 px) and rounded. Each entry is a list of path `d` values. */
const c = (cx: number, cy: number, r: number): string =>
  `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;

export const ICONS = {
  back: ['M15 5l-7 7 7 7'],
  menu: ['M4 7h16', 'M4 12h16', 'M4 17h16'],
  search: [c(11, 11, 6.5), 'M16 16l4.5 4.5'],
  send: ['M12 19V5.5', 'M5.5 11.5L12 5l6.5 6.5'],
  attach: [
    'M20 11.5l-7.8 7.8a4.5 4.5 0 0 1-6.4-6.4l8.1-8.1a3 3 0 0 1 4.3 4.3l-8 8a1.5 1.5 0 0 1-2.1-2.1l7.2-7.2',
  ],
  emoji: [c(12, 12, 9), 'M8.5 14.5a4.5 4.5 0 0 0 7 0', 'M9 9.5h.01', 'M15 9.5h.01'],
  close: ['M6 6l12 12', 'M18 6L6 18'],
  check: ['M5 12.5l4.5 4.5L19 7.5'],
  'check-double': ['M1.5 13l4 4L13 8.5', 'M9.5 16.5l1.5 1.5L22.5 8.5'],
  pin: ['M9 4h6l-1 5 3 3v1.5H7V12l3-3-1-5z', 'M12 13.5V21'],
  archive: ['M3 4h18v4H3z', 'M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8', 'M10 12h4'],
  mute: ['M4 9.5h3.5L12 5.5v13l-4.5-4H4z', 'M16 9l5 6', 'M21 9l-5 6'],
  settings: ['M4 7h10', 'M18 7h2', 'M4 17h2', 'M10 17h10', c(16, 7, 2), c(8, 17, 2)],
  plus: ['M12 5v14', 'M5 12h14'],
  lock: ['M5 13a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2z', 'M8 11V8a4 4 0 0 1 8 0v3'],
  more: ['M5 12h.01', 'M12 12h.01', 'M19 12h.01'],
  grid: ['M4 4.5h7v7H4z', 'M13 4.5h7v7h-7z', 'M4 12.5h7v7H4z', 'M13 12.5h7v7h-7z'],
  reply: ['M10 8L4 13l6 5', 'M4 13h9a7 7 0 0 1 7 7'],
  edit: ['M4 20h4L19 9l-4-4L4 16v4z', 'M13.5 6.5l4 4'],
  trash: ['M4 7h16', 'M9.5 7V4.5h5V7', 'M6.5 7l1 13h9l1-13'],
  copy: [
    'M8 10a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-8a2 2 0 0 1-2-2z',
    'M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2',
  ],
  'chevron-right': ['M9 5l7 7-7 7'],
  qr: ['M4 4h6v6H4z', 'M14 4h6v6h-6z', 'M4 14h6v6H4z', 'M14 14h3v3h-3z', 'M17 17h3v3h-3z'],
  warning: ['M12 3.5L21.5 20h-19L12 3.5z', 'M12 10v4.5', 'M12 17.2h.01'],
} as const;

export type IconName = keyof typeof ICONS;

/** The only icon sizes in the app (DESIGN 4): 20 for inline and list glyphs, 24 for controls. */
export type IconSize = 20 | 24;
