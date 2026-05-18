// Minimal lucide-style icon set. Stroke icons, 1.75 width, 16px default.
const Icon = ({ d, size = 16, stroke = 1.75, className = '', children, fill = 'none', ...rest }) => (
  <svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill={fill} stroke="currentColor" strokeWidth={stroke} strokeLinecap="round" strokeLinejoin="round" className={className} {...rest}>
    {d ? <path d={d} /> : children}
  </svg>
);

const I = {
  // Logo glyph: Tesla-style thin tall "C".
  Claw: ({ size = 18, className = '', ...p }) => (
    <svg xmlns="http://www.w3.org/2000/svg" width={size} height={size * 1.33} viewBox="0 0 18 24" fill="none" className={className} {...p}>
      <path d="M15 3 L5 3 A4 8 0 0 0 5 21 L15 21 L15 18.5 L7 18.5 A3 6.5 0 0 1 7 5.5 L15 5.5 Z" fill="currentColor" />
    </svg>
  ),
  // Big hero mascot — Tesla-style "C" wordmark.
  // Tesla's type cues: tall and narrow, flat terminals, very thin strokes,
  // squared inner corners, slight futuristic geometric precision.
  ClawMascot: ({ size = 120, className = '', ...p }) => (
    <svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 120 160" fill="none" className={className} {...p}>
      {/* Tall narrow C built from outer + inner paths, squared terminals */}
      <path d="
        M 96 28
        L 42 28
        A 34 52 0 0 0 42 132
        L 96 132
        L 96 118
        L 52 118
        A 24 42 0 0 1 52 42
        L 96 42
        Z
      " fill="currentColor" />
    </svg>
  ),
  Home: (p) => <Icon {...p} d="M3 10.5 L12 3 L21 10.5 V20 a1 1 0 0 1 -1 1 h-5 v-7 h-4 v7 H4 a1 1 0 0 1 -1 -1 Z" />,
  Bot: (p) => (<Icon {...p}><rect x="4" y="8" width="16" height="12" rx="2"/><path d="M12 4v4"/><circle cx="9" cy="14" r="1" fill="currentColor"/><circle cx="15" cy="14" r="1" fill="currentColor"/><path d="M9 18h6"/><path d="M2 14h2M20 14h2"/></Icon>),
  Brain: (p) => (<Icon {...p}><path d="M9 4a3 3 0 0 0-3 3v0a3 3 0 0 0-2 5 3 3 0 0 0 2 5 3 3 0 0 0 3 3 3 3 0 0 0 3-3V4Z"/><path d="M15 4a3 3 0 0 1 3 3v0a3 3 0 0 1 2 5 3 3 0 0 1-2 5 3 3 0 0 1-3 3 3 3 0 0 1-3-3V4Z"/></Icon>),
  List: (p) => (<Icon {...p}><path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/></Icon>),
  Activity: (p) => <Icon {...p} d="M3 12h4 l3 -8 l4 16 l3 -8 h4" />,
  Check: (p) => <Icon {...p} d="M5 12l5 5L20 7" />,
  Info: (p) => (<Icon {...p}><circle cx="12" cy="12" r="9"/><path d="M12 8h.01"/><path d="M11 12h1v5h1"/></Icon>),
  Plug: (p) => (<Icon {...p}><path d="M9 2v6M15 2v6M6 8h12v3a6 6 0 0 1-12 0V8zM12 14v8"/></Icon>),
  Radio: (p) => (<Icon {...p}><path d="M4.9 19.1A10 10 0 0 1 4.9 4.9"/><path d="M7.8 16.2a6 6 0 0 1 0-8.5"/><circle cx="12" cy="12" r="2" fill="currentColor" stroke="none"/><path d="M16.2 7.8a6 6 0 0 1 0 8.5"/><path d="M19.1 4.9a10 10 0 0 1 0 14.2"/></Icon>),
  Server: (p) => (<Icon {...p}><rect x="3" y="4" width="18" height="7" rx="1"/><rect x="3" y="13" width="18" height="7" rx="1"/><circle cx="7" cy="7.5" r=".6" fill="currentColor"/><circle cx="7" cy="16.5" r=".6" fill="currentColor"/></Icon>),
  Settings: (p) => (<Icon {...p}><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1A1.6 1.6 0 0 0 4.6 9 1.6 1.6 0 0 0 4.3 7.2l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1Z"/></Icon>),
  Logout: (p) => <Icon {...p} d="M15 4h4a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1h-4 M10 17l-5-5 5-5 M5 12h12" />,
  Search: (p) => (<Icon {...p}><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></Icon>),
  ChevronDown: (p) => <Icon {...p} d="M6 9l6 6 6-6" />,
  ChevronRight: (p) => <Icon {...p} d="M9 6l6 6-6 6" />,
  ChevronLeft: (p) => <Icon {...p} d="M15 6l-6 6 6 6" />,
  ChevronUp: (p) => <Icon {...p} d="M6 15l6-6 6 6" />,
  ChevronsLeft: (p) => (<Icon {...p}><path d="M11 17l-5-5 5-5"/><path d="M18 17l-5-5 5-5"/></Icon>),
  ChevronsRight: (p) => (<Icon {...p}><path d="M13 17l5-5-5-5"/><path d="M6 17l5-5-5-5"/></Icon>),
  Close: (p) => <Icon {...p} d="M18 6L6 18M6 6l12 12" />,
  Plus: (p) => <Icon {...p} d="M12 5v14M5 12h14" />,
  Play: (p) => <Icon {...p} d="M6 4l14 8-14 8V4z" fill="currentColor" stroke="none" />,
  Pause: (p) => (<Icon {...p}><rect x="6" y="4" width="4" height="16" fill="currentColor" stroke="none"/><rect x="14" y="4" width="4" height="16" fill="currentColor" stroke="none"/></Icon>),
  Stop: (p) => <Icon {...p} d="M6 6h12v12H6z" fill="currentColor" stroke="none" />,
  Copy: (p) => (<Icon {...p}><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></Icon>),
  Eye: (p) => (<Icon {...p}><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8S1 12 1 12z"/><circle cx="12" cy="12" r="3"/></Icon>),
  EyeOff: (p) => (<Icon {...p}><path d="M17.94 17.94A11 11 0 0 1 12 20c-7 0-11-8-11-8a18 18 0 0 1 4.06-5.06M9.9 4.24A11 11 0 0 1 12 4c7 0 11 8 11 8a18 18 0 0 1-2.16 3.19M14.12 14.12a3 3 0 1 1-4.24-4.24"/><path d="M2 2l20 20"/></Icon>),
  Filter: (p) => <Icon {...p} d="M22 3H2l8 9.46V19l4 2v-8.54L22 3z" />,
  Refresh: (p) => (<Icon {...p}><path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/><path d="M3 21v-5h5"/></Icon>),
  AlertTriangle: (p) => <Icon {...p} d="M10.3 3.86a2 2 0 0 1 3.4 0l8.1 14.14a2 2 0 0 1-1.7 3H3.9a2 2 0 0 1-1.7-3zM12 9v4M12 17h.01" />,
  CheckCircle: (p) => (<Icon {...p}><circle cx="12" cy="12" r="10"/><path d="M8 12l3 3 5-6"/></Icon>),
  XCircle: (p) => (<Icon {...p}><circle cx="12" cy="12" r="10"/><path d="M15 9l-6 6M9 9l6 6"/></Icon>),
  Clock: (p) => (<Icon {...p}><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></Icon>),
  Circle: (p) => <Icon {...p}><circle cx="12" cy="12" r="9" /></Icon>,
  Dot: (p) => <Icon {...p}><circle cx="12" cy="12" r="4" fill="currentColor" stroke="none" /></Icon>,
  Bell: (p) => (<Icon {...p}><path d="M18 8A6 6 0 1 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/></Icon>),
  Globe: (p) => (<Icon {...p}><circle cx="12" cy="12" r="10"/><path d="M2 12h20M12 2a15 15 0 0 1 0 20M12 2a15 15 0 0 0 0 20"/></Icon>),
  Sun: (p) => (<Icon {...p}><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></Icon>),
  Moon: (p) => <Icon {...p} d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />,
  Zap: (p) => <Icon {...p} d="M13 2L3 14h7l-1 8 10-12h-7l1-8z" fill="currentColor" />,
  User: (p) => (<Icon {...p}><circle cx="12" cy="8" r="4"/><path d="M4 21c0-4.4 3.6-8 8-8s8 3.6 8 8"/></Icon>),
  Shield: (p) => <Icon {...p} d="M12 2l8 3v7c0 5-3.5 8.5-8 10-4.5-1.5-8-5-8-10V5l8-3z" />,
  Key: (p) => (<Icon {...p}><circle cx="7.5" cy="15.5" r="4.5"/><path d="M10.8 12.2L21 2"/><path d="M15 7l3 3M18 4l3 3"/></Icon>),
  Package: (p) => (<Icon {...p}><path d="M21 8l-9-5-9 5 9 5 9-5z"/><path d="M3 8v8l9 5 9-5V8"/><path d="M12 13v8"/></Icon>),
  Terminal: (p) => (<Icon {...p}><path d="M4 17l6-6-6-6M12 19h8"/><rect x="2" y="2" width="20" height="20" rx="2" fill="none" stroke="none"/></Icon>),
  Download: (p) => <Icon {...p} d="M12 3v12m0 0l-5-5m5 5l5-5M5 21h14" />,
  Trash: (p) => (<Icon {...p}><path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></Icon>),
  Edit: (p) => <Icon {...p} d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7M18.5 2.5a2.12 2.12 0 0 1 3 3L12 15l-4 1 1-4z" />,
  Menu: (p) => <Icon {...p} d="M3 6h18M3 12h18M3 18h18" />,
  MoreHorizontal: (p) => (<Icon {...p}><circle cx="5" cy="12" r="1.5" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none"/><circle cx="19" cy="12" r="1.5" fill="currentColor" stroke="none"/></Icon>),
  ArrowRight: (p) => <Icon {...p} d="M5 12h14M13 5l7 7-7 7" />,
  ArrowUpRight: (p) => <Icon {...p} d="M7 17L17 7M8 7h9v9" />,
  Code: (p) => <Icon {...p} d="M16 18l6-6-6-6M8 6l-6 6 6 6" />,
  Database: (p) => (<Icon {...p}><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v6c0 1.66 4 3 9 3s9-1.34 9-3V5"/><path d="M3 11v6c0 1.66 4 3 9 3s9-1.34 9-3v-6"/></Icon>),
  MessageSquare: (p) => <Icon {...p} d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />,
  Hash: (p) => <Icon {...p} d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 18" />,
  Book: (p) => (<Icon {...p}><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/></Icon>),
  Layers: (p) => <Icon {...p} d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />,
  Sliders: (p) => <Icon {...p} d="M4 21V14M4 10V3M12 21V12M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" />,
  X: (p) => <Icon {...p} d="M18 6L6 18M6 6l12 12" />,
  Send: (p) => <Icon {...p} d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z" />,
  Flag: (p) => <Icon {...p} d="M4 22V4s0-2 2-2c2 0 2 2 6 2s4-2 8-2 4 2 4 2v12s0 2-4 2-4-2-8-2-6 2-6 2" />,
  Loader: (p) => (<Icon {...p}><path d="M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.2 16.2l2.8 2.8M2 12h4M18 12h4M4.9 19.1l2.8-2.8M16.2 7.8l2.8-2.8" opacity=".8"/></Icon>),
  Wifi: (p) => (<Icon {...p}><path d="M5 13a10 10 0 0 1 14 0"/><path d="M8.5 16.5a5 5 0 0 1 7 0"/><path d="M12 20h.01"/><path d="M2 9a16 16 0 0 1 20 0"/></Icon>),
  Cpu: (p) => (<Icon {...p}><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/><path d="M9 2v2M15 2v2M9 20v2M15 20v2M2 9h2M2 15h2M20 9h2M20 15h2"/></Icon>),
  // Sprint 11 Admin UI additions (Wave 1 A1/A2 agents used these but
  // forgot to register; QA regression caught the crash).
  Save: (p) => (<Icon {...p}><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><polyline points="17 21 17 13 7 13 7 21"/><polyline points="7 3 7 8 15 8"/></Icon>),
  ArrowLeft: (p) => <Icon {...p} d="M19 12H5M12 19l-7-7 7-7" />,
  Mic: (p) => (<Icon {...p}><rect x="9" y="3" width="6" height="12" rx="3"/><path d="M19 10a7 7 0 0 1-14 0"/><path d="M12 17v4M8 21h8"/></Icon>),
  Sparkles: (p) => (<Icon {...p}><path d="M12 3l1.5 4.5L18 9l-4.5 1.5L12 15l-1.5-4.5L6 9l4.5-1.5L12 3z"/><path d="M5 17l.75 2.25L8 20l-2.25.75L5 23l-.75-2.25L2 20l2.25-.75L5 17z"/><path d="M19 17l.5 1.5L21 19l-1.5.5L19 21l-.5-1.5L17 19l1.5-.5L19 17z"/></Icon>),
  Folder: (p) => (<Icon {...p}><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></Icon>),
  // F4 — cluster / brain node icons
  Cluster: (p) => (<Icon {...p}><circle cx="12" cy="5" r="2"/><circle cx="5" cy="19" r="2"/><circle cx="19" cy="19" r="2"/><path d="M12 7v3M12 10l-5.5 7M12 10l5.5 7"/></Icon>),
  BrainNode: (p) => (<Icon {...p}><rect x="3" y="8" width="7" height="8" rx="1.5"/><rect x="14" y="8" width="7" height="8" rx="1.5"/><path d="M10 12h4"/><path d="M6.5 4v4M17.5 4v4M6.5 16v4M17.5 16v4"/></Icon>),
  // F7 — tools state icon
  ToolState: (p) => (<Icon {...p}><rect x="2" y="3" width="20" height="5" rx="1"/><rect x="2" y="10" width="12" height="5" rx="1"/><path d="M16 12.5h6M19 10v5"/></Icon>),
  // F3 — capability discover icon
  Discover: (p) => (<Icon {...p}><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/><path d="M11 8v6M8 11h6"/></Icon>),
  // F5 — delegate / sub-agent icon
  Delegate: (p) => (<Icon {...p}><circle cx="5" cy="12" r="2"/><circle cx="19" cy="6" r="2"/><circle cx="19" cy="18" r="2"/><path d="M7 12h8a2 2 0 0 0 2-2V8M7 12h8a2 2 0 0 0 2 2v0"/><path d="M7 12l10-4M7 12l10 4"/></Icon>),
  // F6 — MCP bridge icon
  Bridge: (p) => (<Icon {...p}><path d="M4 10h16"/><path d="M4 14h16"/><path d="M8 6v12M16 6v12"/><rect x="2" y="4" width="4" height="4" rx="1"/><rect x="18" y="4" width="4" height="4" rx="1"/><rect x="2" y="16" width="4" height="4" rx="1"/><rect x="18" y="16" width="4" height="4" rx="1"/></Icon>),
};

Object.assign(window, { I, Icon });
