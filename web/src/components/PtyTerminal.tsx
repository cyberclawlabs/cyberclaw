// PtyTerminal — xterm.js WebSocket bridge.
//
// Mounts an xterm.js terminal and connects it to the backend PTY WebSocket.
// Binary frames from the server are written to the terminal; user keystrokes
// are sent as binary frames. Resize events are sent as JSON text frames.
//
// The terminal mounts only when this component is rendered (lazy connect).
// Cleanup closes the WebSocket and disposes xterm on unmount.

import { useEffect, useRef } from "react";

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
      const { Terminal } = await import("xterm");
      const { FitAddon } = await import("xterm-addon-fit");

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
      });

      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(containerRef.current);
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
