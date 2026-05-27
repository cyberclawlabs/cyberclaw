// PtyTerminal — xterm.js WebSocket bridge.
//
// Mounts an xterm.js terminal and connects it to the backend PTY WebSocket.
// Binary frames from the server are written to the terminal; user keystrokes
// are sent as binary frames. Resize events are sent as JSON text frames.
//
// The terminal mounts only when this component is rendered (lazy connect).
// Cleanup closes the WebSocket and disposes xterm on unmount.
//
// v1.4 W2: migrated from deprecated `xterm`/`xterm-addon-fit` to `@xterm/*`
// scoped packages. Added Unicode11 (CJK perf), WebGL (rendering perf), and
// WebLinks (clickable URLs) addons — feature parity with hermes-agent/web.

import { useEffect, useRef } from "react";
// xterm.css is imported globally in main.tsx

interface PtyTerminalProps {
  wsUrl: string;
}

export default function PtyTerminal({ wsUrl }: PtyTerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    let disposed = false;

    // Cleanup reference — updated by the async block once setup is done.
    // If the component unmounts before setup completes, disposed=true stops it.
    let cleanup: (() => void) | undefined;

    void (async () => {
      const [{ Terminal }, { FitAddon }, { Unicode11Addon }, { WebLinksAddon }, { WebglAddon }] =
        await Promise.all([
          import("@xterm/xterm"),
          import("@xterm/addon-fit"),
          import("@xterm/addon-unicode11"),
          import("@xterm/addon-web-links"),
          import("@xterm/addon-webgl"),
        ]);

      if (disposed || !containerRef.current) return;

      const term = new Terminal({
        fontFamily: '"JetBrains Mono", "Fira Mono", monospace',
        fontSize: 13,
        theme: {
          background: "#0a0a0b",
          foreground: "#f4f4f5",
          cursor: "#a1a1aa",
          selectionBackground: "#3f3f46",
        },
        cursorBlink: true,
        scrollback: 1000,
        allowProposedApi: true, // required for Unicode11Addon
      });

      const fit = new FitAddon();
      term.loadAddon(fit);
      term.loadAddon(new Unicode11Addon()); // CJK / emoji width fix
      term.unicode.activeVersion = "11";
      term.loadAddon(new WebLinksAddon()); // clickable URLs
      term.open(containerRef.current);
      // WebGL must be loaded AFTER open() — falls back to canvas if WebGL fails
      try {
        const webgl = new WebglAddon();
        webgl.onContextLoss(() => webgl.dispose());
        term.loadAddon(webgl);
      } catch {
        // WebGL unsupported → silent fallback to canvas renderer
      }
      fit.fit();

      const ws = new WebSocket(wsUrl);
      ws.binaryType = "arraybuffer";

      ws.onopen = () => {
        // Send initial terminal size after connection.
        ws.send(
          JSON.stringify({ type: "resize", cols: term.cols, rows: term.rows }),
        );
      };

      ws.onmessage = (e) => {
        if (e.data instanceof ArrayBuffer) {
          term.write(new Uint8Array(e.data));
        }
        // Text control frames are intentionally ignored (server sends none in v1).
      };

      ws.onerror = () => {
        term.write("\r\n\x1b[31m[WebSocket error — connection closed]\x1b[0m\r\n");
      };

      ws.onclose = () => {
        if (!disposed) {
          term.write("\r\n\x1b[33m[Session ended]\x1b[0m\r\n");
        }
      };

      // User keystrokes → PTY stdin (binary frame).
      term.onData((data) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(new TextEncoder().encode(data));
        }
      });

      // Terminal resize → PTY resize (JSON text frame).
      term.onResize(({ cols, rows }) => {
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: "resize", cols, rows }));
        }
      });

      const onWindowResize = () => {
        fit.fit();
      };
      window.addEventListener("resize", onWindowResize);

      cleanup = () => {
        disposed = true;
        window.removeEventListener("resize", onWindowResize);
        ws.close();
        term.dispose();
      };
    })();

    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [wsUrl]);

  return (
    <div
      ref={containerRef}
      className="w-full h-full"
      style={{ background: "#0a0a0b" }}
    />
  );
}
