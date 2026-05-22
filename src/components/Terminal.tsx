import { useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { ipc, onTerminalData, onTerminalExit } from "@/lib/ipc";
import type { SessionId } from "@/lib/types";
import { useSettings, TERMINAL_THEMES } from "@/stores/settingsStore";

interface Props {
  sessionId: SessionId;
}

export function Terminal({ sessionId }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const terminalIdRef = useRef<string | null>(null);
  const [status, setStatus] = useState<"idle" | "opening" | "ready" | "exited">(
    "idle"
  );
  const [error, setError] = useState<string | null>(null);

  const fontSize = useSettings((s) => s.terminalFontSize);
  const fontFamily = useSettings((s) => s.terminalFontFamily);
  const themeKey = useSettings((s) => s.terminalTheme);
  const scrollback = useSettings((s) => s.terminalScrollback);

  // Mount the terminal once per session. Settings other than scrollback are
  // applied live in a separate effect below.
  useEffect(() => {
    if (!containerRef.current) return;

    const { terminalFontSize, terminalFontFamily, terminalTheme, terminalScrollback } =
      useSettings.getState();
    const term = new XTerm({
      fontFamily: terminalFontFamily,
      fontSize: terminalFontSize,
      scrollback: terminalScrollback,
      theme: TERMINAL_THEMES[terminalTheme],
      cursorBlink: true,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);
    fit.fit();
    term.focus();
    termRef.current = term;
    fitRef.current = fit;

    let unlistenData: (() => void) | null = null;
    let unlistenExit: (() => void) | null = null;
    let disposed = false;

    const dataDisposable = term.onData((data) => {
      const id = terminalIdRef.current;
      if (id) ipc.terminalWrite(id, data).catch(() => {});
    });

    const resizeDisposable = term.onResize(({ cols, rows }) => {
      const id = terminalIdRef.current;
      if (id) ipc.terminalResize(id, cols, rows).catch(() => {});
    });

    const onWindowResize = () => {
      try {
        fit.fit();
      } catch {}
    };
    window.addEventListener("resize", onWindowResize);

    (async () => {
      try {
        setStatus("opening");
        unlistenData = await onTerminalData((e) => {
          if (e.terminalId === terminalIdRef.current) term.write(e.data);
        });
        unlistenExit = await onTerminalExit((e) => {
          if (e.terminalId === terminalIdRef.current) {
            setStatus("exited");
            term.writeln(
              `\r\n\x1b[33m[session ended${
                e.code !== null ? ` (exit ${e.code})` : ""
              }]\x1b[0m`
            );
          }
        });
        const id = await ipc.openTerminal(sessionId, term.cols, term.rows);
        terminalIdRef.current = id;
        if (disposed) {
          ipc.closeTerminal(id).catch(() => {});
          return;
        }
        setStatus("ready");
        // Some webviews lose focus between term.open() and openTerminal returning.
        // Re-focus once the backend is wired up so the very first keystroke lands.
        term.focus();
      } catch (e) {
        setError(String(e));
        setStatus("exited");
      }
    })();

    return () => {
      disposed = true;
      window.removeEventListener("resize", onWindowResize);
      dataDisposable.dispose();
      resizeDisposable.dispose();
      if (unlistenData) unlistenData();
      if (unlistenExit) unlistenExit();
      const id = terminalIdRef.current;
      if (id) ipc.closeTerminal(id).catch(() => {});
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      terminalIdRef.current = null;
    };
  }, [sessionId]);

  // Live-apply font/theme changes without recreating the term.
  useEffect(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term) return;
    term.options.fontSize = fontSize;
    term.options.fontFamily = fontFamily;
    term.options.theme = TERMINAL_THEMES[themeKey];
    if (fit) {
      try {
        fit.fit();
      } catch {}
    }
  }, [fontSize, fontFamily, themeKey]);

  // Scrollback change requires a new buffer; we just note it for clarity.
  useEffect(() => {
    // Intentional: scrollback only takes effect on next terminal session.
  }, [scrollback]);

  const theme = TERMINAL_THEMES[themeKey];

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 border-b border-border bg-bg-subtle px-3 py-1.5">
        <span className="text-xs font-semibold uppercase tracking-wider text-text-muted">
          Terminal
        </span>
        <span className="text-xs text-text-dim">
          {status === "opening" && "opening…"}
          {status === "ready" && "connected"}
          {status === "exited" && "exited"}
        </span>
      </div>
      <div
        ref={containerRef}
        onMouseDown={() => termRef.current?.focus()}
        className="h-full w-full flex-1 overflow-hidden"
        style={{ background: theme.background }}
      />
      {error && (
        <div className="border-t border-border bg-danger-soft px-3 py-1 text-xs text-danger">
          {error}
        </div>
      )}
    </div>
  );
}
