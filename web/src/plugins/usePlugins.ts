// usePlugins — central source-of-truth React hook for Platform Plugin manifests.
//
// Loads `GET /api/v1/plugins` on mount, exposes the parsed list + derived
// "ready to mount in the shell" subset (enabled + tab_path present + not
// hidden). Sidebar + AppV2 consume the same hook so a plugin appears in
// nav and as a route in lockstep — no manual wiring per plugin.
//
// Hot-reload: call `refresh()` after dropping a new plugin.json (or invoke
// /api/v1/plugins/rescan and then refresh()). UI does NOT auto-poll because
// platform plugins are local-filesystem only and rarely change at runtime.

import { useEffect, useState, useCallback } from "react";
import { type Plugin, fetchPlugins } from "@/lib/api";

/// A plugin entry that the shell is willing to dynamically mount as a nav
/// item + route. Plugins missing tab_path or marked hidden / disabled are
/// returned by `plugins` but excluded from `mountable`.
export interface MountablePlugin {
  name: string;
  label: string;
  tab_path: string;
  icon?: string;
  description?: string;
  version: string;
}

export interface UsePluginsResult {
  plugins: Plugin[];
  mountable: MountablePlugin[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

export function usePlugins(): UsePluginsResult {
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await fetchPlugins();
      setPlugins(list);
    } catch (e) {
      // Plugin loading failure must NOT break the shell — log and continue
      // with an empty list. The user sees the empty Plugins page; the rest
      // of the SPA works normally.
      // eslint-disable-next-line no-console
      console.error("usePlugins: fetch failed", e);
      setError(e instanceof Error ? e.message : String(e));
      setPlugins([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const mountable: MountablePlugin[] = plugins
    .filter((p) => p.enabled && !p.hidden && typeof p.tab_path === "string" && p.tab_path.length > 0)
    .map((p) => ({
      name: p.name,
      label: p.label || p.name,
      tab_path: p.tab_path as string,
      icon: p.icon ?? undefined,
      description: p.description ?? undefined,
      version: p.version,
    }));

  return { plugins, mountable, loading, error, refresh };
}
