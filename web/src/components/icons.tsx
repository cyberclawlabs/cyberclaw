// Lucide-style SVG icon set for v2 Sidebar. 1.75 stroke, 24×24 viewBox, currentColor.
import type { ReactNode } from "react";

type IconProps = {
  size?: number;
  stroke?: number;
  className?: string;
  fill?: string;
};

const Icon = ({
  d,
  size = 16,
  stroke = 1.75,
  className = "",
  fill = "none",
  children,
}: IconProps & { d?: string; children?: ReactNode }) => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill={fill}
    stroke="currentColor"
    strokeWidth={stroke}
    strokeLinecap="round"
    strokeLinejoin="round"
    className={className}
  >
    {d ? <path d={d} /> : children}
  </svg>
);

export const Claw = ({ size = 18, className = "" }: IconProps) => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    width={size}
    height={size * 1.33}
    viewBox="0 0 18 24"
    fill="none"
    className={className}
  >
    <path
      d="M15 3 L5 3 A4 8 0 0 0 5 21 L15 21 L15 18.5 L7 18.5 A3 6.5 0 0 1 7 5.5 L15 5.5 Z"
      fill="currentColor"
    />
  </svg>
);

export const Home = (p: IconProps) => (
  <Icon {...p} d="M3 10.5 L12 3 L21 10.5 V20 a1 1 0 0 1 -1 1 h-5 v-7 h-4 v7 H4 a1 1 0 0 1 -1 -1 Z" />
);

export const MessageSquare = (p: IconProps) => (
  <Icon {...p} d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
);

export const HelpCircle = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="10" />
    <path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
    <path d="M12 17h.01" />
  </Icon>
);

export const ArrowRight = (p: IconProps) => (
  <Icon {...p} d="M5 12h14M13 5l7 7-7 7" />
);

export const Bot = (p: IconProps) => (
  <Icon {...p}>
    <rect x="4" y="8" width="16" height="12" rx="2" />
    <path d="M12 4v4" />
    <circle cx="9" cy="14" r="1" fill="currentColor" />
    <circle cx="15" cy="14" r="1" fill="currentColor" />
    <path d="M9 18h6" />
    <path d="M2 14h2M20 14h2" />
  </Icon>
);

export const Brain = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 4a3 3 0 0 0-3 3v0a3 3 0 0 0-2 5 3 3 0 0 0 2 5 3 3 0 0 0 3 3 3 3 0 0 0 3-3V4Z" />
    <path d="M15 4a3 3 0 0 1 3 3v0a3 3 0 0 1 2 5 3 3 0 0 1-2 5 3 3 0 0 1-3 3 3 3 0 0 1-3-3V4Z" />
  </Icon>
);

export const Database = (p: IconProps) => (
  <Icon {...p}>
    <ellipse cx="12" cy="5" rx="9" ry="3" />
    <path d="M3 5v6c0 1.66 4 3 9 3s9-1.34 9-3V5" />
    <path d="M3 11v6c0 1.66 4 3 9 3s9-1.34 9-3v-6" />
  </Icon>
);

export const Paperclip = (p: IconProps) => (
  <Icon {...p}>
    <path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57a4 4 0 0 1 5.66 5.66l-8.58 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48" />
  </Icon>
);

export const User = (p: IconProps) => (
  <Icon {...p}>
    <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
    <circle cx="12" cy="7" r="4" />
  </Icon>
);

export const BookOpen = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z" />
    <path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z" />
  </Icon>
);

export const CheckCircle = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="10" />
    <path d="M8 12l3 3 5-6" />
  </Icon>
);

export const List = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
  </Icon>
);

export const Activity = (p: IconProps) => (
  <Icon {...p} d="M3 12h4 l3 -8 l4 16 l3 -8 h4" />
);

export const Check = (p: IconProps) => (
  <Icon {...p} d="M5 12l5 5L20 7" />
);

export const Columns = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="3" width="5" height="18" rx="1" />
    <rect x="10" y="3" width="5" height="18" rx="1" />
    <rect x="17" y="3" width="4" height="18" rx="1" />
  </Icon>
);

export const Terminal = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4 17l6-6-6-6" />
    <path d="M12 19h8" />
  </Icon>
);

export const Cpu = (p: IconProps) => (
  <Icon {...p}>
    <rect x="4" y="4" width="16" height="16" rx="2" />
    <rect x="9" y="9" width="6" height="6" />
    <path d="M9 2v2M15 2v2M9 20v2M15 20v2M2 9h2M2 15h2M20 9h2M20 15h2" />
  </Icon>
);

export const Clock = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="10" />
    <path d="M12 6v6l4 2" />
  </Icon>
);

export const Plug = (p: IconProps) => (
  <Icon {...p}>
    <path d="M9 2v6M15 2v6M6 8h12v3a6 6 0 0 1-12 0V8zM12 14v8" />
  </Icon>
);

export const AlertTriangle = (p: IconProps) => (
  <Icon {...p} d="M10.3 3.86a2 2 0 0 1 3.4 0l8.1 14.14a2 2 0 0 1-1.7 3H3.9a2 2 0 0 1-1.7-3zM12 9v4M12 17h.01" />
);

export const Wrench = (p: IconProps) => (
  <Icon {...p}>
    <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
  </Icon>
);

export const Puzzle = (p: IconProps) => (
  <Icon {...p}>
    <path d="M19 9V7a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h3" />
    <path d="M13 15a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2h-4a2 2 0 0 1-2-2v-4z" />
    <path d="M10 9H7" />
    <path d="M10 13H7" />
  </Icon>
);

export const Radio = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4.9 19.1A10 10 0 0 1 4.9 4.9" />
    <path d="M7.8 16.2a6 6 0 0 1 0-8.5" />
    <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none" />
    <path d="M16.2 7.8a6 6 0 0 1 0 8.5" />
    <path d="M19.1 4.9a10 10 0 0 1 0 14.2" />
  </Icon>
);

export const MessageCircle = (p: IconProps) => (
  <Icon {...p} d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" />
);

export const Image = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <circle cx="8.5" cy="8.5" r="1.5" />
    <path d="M21 15l-5-5L5 21" />
  </Icon>
);

export const Layers = (p: IconProps) => (
  <Icon {...p} d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
);

export const Globe = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="10" />
    <path d="M2 12h20M12 2a15 15 0 0 1 0 20M12 2a15 15 0 0 0 0 20" />
  </Icon>
);

export const Shield = (p: IconProps) => (
  <Icon {...p} d="M12 2l8 3v7c0 5-3.5 8.5-8 10-4.5-1.5-8-5-8-10V5l8-3z" />
);

export const FileText = (p: IconProps) => (
  <Icon {...p}>
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
    <path d="M14 2v6h6" />
    <path d="M16 13H8M16 17H8M10 9H8" />
  </Icon>
);

export const Server = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="4" width="18" height="7" rx="1" />
    <rect x="3" y="13" width="18" height="7" rx="1" />
    <circle cx="7" cy="7.5" r=".6" fill="currentColor" />
    <circle cx="7" cy="16.5" r=".6" fill="currentColor" />
  </Icon>
);

export const Network = (p: IconProps) => (
  <Icon {...p}>
    <rect x="9" y="2" width="6" height="4" rx="1" />
    <rect x="2" y="18" width="6" height="4" rx="1" />
    <rect x="16" y="18" width="6" height="4" rx="1" />
    <path d="M12 6v4M5 18v-4h14v4" />
    <path d="M12 10v4" />
  </Icon>
);

export const Users = (p: IconProps) => (
  <Icon {...p}>
    <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
    <circle cx="9" cy="7" r="4" />
    <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
  </Icon>
);

export const Settings = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.8-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.8 1.6 1.6 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1A1.6 1.6 0 0 0 4.6 9 1.6 1.6 0 0 0 4.3 7.2l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.6 1.6 0 0 0 1 1.5 1.6 1.6 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.8V9a1.6 1.6 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1Z" />
  </Icon>
);
