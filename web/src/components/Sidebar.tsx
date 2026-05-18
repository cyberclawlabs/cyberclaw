// Sidebar — primary navigation
import { NavLink } from "react-router-dom";
import type { ComponentType } from "react";
import { type Lang } from "@/lib/i18n";
import {
  Activity, BookOpen, Bot, Brain, Check, CheckCircle, Claw, Clock, Cpu,
  Database, FileText, Globe, HelpCircle, Home, Layers, List, MessageCircle,
  MessageSquare, Network, Plug, Puzzle, Radio, Server, Settings, Shield,
  Terminal, User, Users, Wrench,
} from "./icons";

type IconComponent = ComponentType<{ size?: number; className?: string }>;
type NavItem = { path: string; labelEn: string; labelZh: string; icon: IconComponent };
type NavGroup = { titleEn: string; titleZh: string; items: NavItem[] };

const GROUPS: NavGroup[] = [
  {
    titleEn: "Conversations",
    titleZh: "会话",
    items: [
      // P1-H icon dedupe: each route gets a visually distinct icon. Before
      // this pass, /sessions+/profiles+/im-platforms+/me all shared
      // MessageCircle, /audit+/security shared Shield, /docs+/logs shared
      // FileText, /tasks+/sessions shared List, etc. — operators relying on
      // icon shape for quick scan had to read every label.
      { path: "/", labelEn: "Status", labelZh: "状态", icon: Home},
      { path: "/chat", labelEn: "Chat", labelZh: "对话", icon: MessageSquare},
      { path: "/sessions", labelEn: "Sessions", labelZh: "会话列表", icon: List},
      { path: "/profiles", labelEn: "Profiles", labelZh: "配置档案", icon: Users},
      { path: "/clarifications", labelEn: "Clarifications", labelZh: "澄清请求", icon: HelpCircle},
    ],
  },
  {
    titleEn: "Knowledge",
    titleZh: "知识与能力",
    items: [
      { path: "/agents", labelEn: "Agents & Handoffs", labelZh: "智能体与协作", icon: Bot},
      { path: "/skills", labelEn: "Skills", labelZh: "技能", icon: Wrench},
      { path: "/memory", labelEn: "Memory", labelZh: "记忆", icon: Database},
      { path: "/learning", labelEn: "Learning & Curator", labelZh: "学习与归纳", icon: Brain},
    ],
  },
  {
    titleEn: "Operations",
    titleZh: "执行与运营",
    items: [
      { path: "/tasks", labelEn: "Tasks & Executions", labelZh: "任务与执行", icon: CheckCircle},
      { path: "/reviews", labelEn: "Reviews", labelZh: "审批", icon: Check},
      { path: "/cron", labelEn: "Cron", labelZh: "定时任务", icon: Clock},
      { path: "/models", labelEn: "Models", labelZh: "模型", icon: Cpu},
      { path: "/moa", labelEn: "Mixture-of-Agents", labelZh: "智能体混合编排", icon: Layers},
    ],
  },
  {
    titleEn: "Platform",
    titleZh: "平台生态",
    items: [
      { path: "/capabilities", labelEn: "Capabilities & Tools", labelZh: "能力与工具", icon: Plug},
      { path: "/plugins", labelEn: "Plugins", labelZh: "平台插件", icon: Puzzle},
      { path: "/channels", labelEn: "Channels", labelZh: "渠道", icon: Radio},
      { path: "/im-platforms", labelEn: "IM Platforms", labelZh: "即时通讯平台", icon: Globe},
    ],
  },
  {
    titleEn: "Admin",
    titleZh: "管理",
    items: [
      { path: "/overview", labelEn: "Overview", labelZh: "系统介绍", icon: BookOpen },
      { path: "/docs", labelEn: "Docs", labelZh: "文档", icon: FileText },
      // Uploads page intentionally NOT exposed in the sidebar — file uploads
      // are now driven by the Chat paperclip (Composer). The /uploads route
      // is kept in AppV2 for backward links + admin debugging only.
      { path: "/audit", labelEn: "Audit", labelZh: "审计", icon: Activity},
      { path: "/trace", labelEn: "Trace", labelZh: "追踪", icon: Network},
      { path: "/security", labelEn: "Security", labelZh: "安全", icon: Shield},
      { path: "/logs", labelEn: "Logs", labelZh: "日志", icon: Terminal},
      { path: "/nodes", labelEn: "Cluster", labelZh: "集群节点", icon: Server},
      { path: "/settings", labelEn: "Settings", labelZh: "设置", icon: Settings},
      { path: "/me", labelEn: "My Account", labelZh: "我的账户", icon: User},
    ],
  },
];

/// Dynamic nav items injected from Platform Plugin manifests. Mounted by
/// AppV2 via `usePlugins().mountable`. Keeps Sidebar pure / stateless —
/// the hook lives one level up.
export interface PluginNavItem {
  path: string;
  label: string;
  icon?: string;
}

export default function Sidebar({
  collapsed,
  version,
  lang,
  pluginItems = [],
}: {
  collapsed: boolean;
  version: string;
  lang: Lang;
  pluginItems?: PluginNavItem[];
}) {
  return (
    <aside
      className="border-r border-border flex flex-col bg-bg-2 shrink-0"
      style={{ width: collapsed ? 64 : 240, transition: "width .18s ease" }}
    >
      <NavLink
        to="/"
        className={`h-11 border-b border-border flex items-center ${collapsed ? "justify-center" : "px-4"} shrink-0 hover:bg-bg-2 transition-colors`}
        title="CyberClaw — 回到主页"
      >
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-accent shrink-0 leading-none">
            <Claw size={18} />
          </span>
          {!collapsed && (
            <div className="min-w-0">
              <div className="text-[13px] font-semibold tracking-tight leading-none">
                CyberClaw
              </div>
              <div className="text-[10px] text-fg-4 mono leading-none mt-1">
                v{version}
              </div>
            </div>
          )}
        </div>
      </NavLink>
      <nav className="flex-1 py-2 overflow-auto">
        {GROUPS.map((g) => (
          <div key={g.titleEn} className="mb-3">
            {!collapsed && (
              <div className="px-5 pt-2 pb-1 text-[10px] mono uppercase tracking-[0.14em] text-fg-4">
                {lang === "zh-CN" ? g.titleZh : g.titleEn}
              </div>
            )}
            {g.items.map((item) => (
              <NavItem key={item.path} item={item} collapsed={collapsed} lang={lang} />
            ))}
          </div>
        ))}
        {pluginItems.length > 0 && (
          <div className="mb-3">
            {!collapsed && (
              <div className="px-5 pt-2 pb-1 text-[10px] mono uppercase tracking-[0.14em] text-fg-4">
                {lang === "zh-CN" ? "插件" : "Plugins"}
              </div>
            )}
            {pluginItems.map((p) => (
              <PluginNav key={p.path} item={p} collapsed={collapsed} />
            ))}
          </div>
        )}
      </nav>
    </aside>
  );
}

function PluginNav({ item, collapsed }: { item: PluginNavItem; collapsed: boolean }) {
  return (
    <NavLink
      to={item.path}
      title={collapsed ? item.label : undefined}
      className={({ isActive }) =>
        [
          "w-full flex items-center gap-2.5 h-8 mx-2 my-0.5 rounded-md text-[13px] transition-colors",
          collapsed ? "justify-center px-0" : "px-3",
          isActive
            ? "bg-bg-3 text-fg border border-border"
            : "text-fg-2 hover:bg-hover hover:text-fg border border-transparent",
        ].join(" ")
      }
      style={{ width: collapsed ? 48 : "calc(100% - 16px)" }}
    >
      <span className="shrink-0 leading-none text-accent text-[14px]">
        {item.icon || <Puzzle size={16} />}
      </span>
      {!collapsed && (
        <span className="truncate flex-1 text-left">{item.label}</span>
      )}
    </NavLink>
  );
}

function NavItem({ item, collapsed, lang }: { item: NavItem; collapsed: boolean; lang: Lang }) {
  const label = lang === "zh-CN" ? item.labelZh : item.labelEn;
  return (
    <NavLink
      to={item.path}
      end={item.path === "/"}
      title={collapsed ? label : undefined}
      className={({ isActive }) =>
        [
          "w-full flex items-center gap-2.5 h-8 mx-2 my-0.5 rounded-md text-[13px] transition-colors",
          collapsed ? "justify-center px-0" : "px-3",
          isActive
            ? "bg-bg-3 text-fg border border-border"
            : "text-fg-2 hover:bg-hover hover:text-fg border border-transparent",
        ].join(" ")
      }
      style={{ width: collapsed ? 48 : "calc(100% - 16px)" }}
    >
      <span className="shrink-0 leading-none text-accent">
        <item.icon size={16} />
      </span>
      {!collapsed && (
        <span className="truncate flex-1 text-left">{label}</span>
      )}
    </NavLink>
  );
}

export function getAllPaths(): { path: string; label: string }[] {
  return GROUPS.flatMap((g) => g.items).map((i) => ({ path: i.path, label: i.labelEn }));
}
