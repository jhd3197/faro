import { useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Plus, X, TerminalSquare } from "lucide-react";
import { ipc, onTerminalData, onTerminalExit } from "@/lib/ipc";
import type { SessionId } from "@/lib/types";
import { useSettings, TERMINAL_THEMES } from "@/stores/settingsStore";
import { useTerminals, type TerminalTab } from "@/stores/terminalsStore";
import { cn } from "@/lib/cn";

/// Tabbed terminal dock. Backend supports N PTY channels per session — we
/// open one per tab and keep each `<TerminalPane>` mounted so switching tabs
/// is instant and doesn't drop the shell. All panes for non-active tabs are
/// hidden via CSS rather than unmounted.
export function TerminalDock({ sessionId }: { sessionId: SessionId }) {
  const tabs = useTerminals((s) => s.tabs);
  const activeId = useTerminals((s) => s.activeId);
  const openTab = useTerminals((s) => s.openTab);
  const closeTab = useTerminals((s) => s.closeTab);
  const setActive = useTerminals((s) => s.setActive);
  const renameTab = useTerminals((s) => s.renameTab);
  const dropSessionTabs = useTerminals((s) => s.dropSessionTabs);

  const sessionTabs = tabs.filter((t) => t.sessionId === sessionId);

  // Open a first tab when the dock first becomes visible for this session.
  // We only run this when sessionTabs is empty; a no-op otherwise.
  useEffect(() => {
    if (sessionTabs.length === 0) {
      openTab(sessionId);
    }
  }, [sessionId, sessionTabs.length, openTab]);

  // If the active tab belongs to a different session, switch to one in ours.
  useEffect(() => {
    if (activeId && !sessionTabs.some((t) => t.id === activeId)) {
      const first = sessionTabs[0];
      if (first) setActive(first.id);
    }
  }, [activeId, sessionTabs, setActive]);

  // When the session disconnects (sessionId changes), drop tabs from prior
  // sessions so they don't linger. We can't watch "disconnect" directly, so we
  // rely on the parent unmounting this dock when there's no session.
  useEffect(() => {
    return () => {
      // On unmount, drop our session's tabs.
      dropSessionTabs(sessionId);
    };
  }, [sessionId, dropSessionTabs]);

  const closing = (id: string) => {
    closeTab(id);
    if (tabs.length <= 1) {
      // Last tab closed — close the dock so we don't show an empty bar.
      // The parent watches `terminalOpen`; we don't toggle it from here.
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border bg-bg-subtle px-2">
        <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {sessionTabs.map((tab) => (
            <TabChip
              key={tab.id}
              tab={tab}
              active={tab.id === activeId}
              onClick={() => setActive(tab.id)}
              onClose={() => closing(tab.id)}
              onRename={(title) => renameTab(tab.id, title)}
            />
          ))}
          <button
            onClick={() => openTab(sessionId)}
            title="New shell"
            className="ml-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <Plus size={12} />
          </button>
        </div>
      </div>
      <div className="relative flex-1 overflow-hidden">
        {sessionTabs.map((tab) => (
          <TerminalPane
            key={tab.id}
            tab={tab}
            visible={tab.id === activeId}
          />
        ))}
      </div>
    </div>
  );
}

function TabChip({
  tab,
  active,
  onClick,
  onClose,
  onRename,
}: {
  tab: TerminalTab;
  active: boolean;
  onClick: () => void;
  onClose: () => void;
  onRename: (title: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(tab.title);

  if (editing) {
    return (
      <div
        className={cn(
          "flex h-6 shrink-0 items-center gap-1 rounded-md px-1.5 ring-1 ring-inset",
          active ? "bg-bg-panel ring-accent/40" : "bg-bg-panel ring-border"
        )}
      >
        <TerminalSquare size={11} className="text-text-muted" />
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => {
            onRename(draft.trim() || tab.title);
            setEditing(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              onRename(draft.trim() || tab.title);
              setEditing(false);
            } else if (e.key === "Escape") {
              setDraft(tab.title);
              setEditing(false);
            }
          }}
          className="w-20 bg-transparent text-[11px] outline-none"
        />
      </div>
    );
  }

  return (
    <button
      onClick={onClick}
      onDoubleClick={() => {
        setDraft(tab.title);
        setEditing(true);
      }}
      title={`${tab.title} — double-click to rename`}
      className={cn(
        "group/tab flex h-6 shrink-0 items-center gap-1.5 rounded-md px-2 text-[11px] transition-colors",
        active
          ? "bg-bg-panel text-text ring-1 ring-inset ring-accent/30"
          : "text-text-muted hover:bg-bg-hover hover:text-text"
      )}
    >
      <TerminalSquare size={11} className={cn("shrink-0", active && "text-accent")} />
      <span className="max-w-[18rem] truncate">{tab.title}</span>
      <span
        role="button"
        tabIndex={-1}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className="ml-0.5 flex h-4 w-4 items-center justify-center rounded text-text-dim opacity-0 hover:bg-bg-hover hover:text-text group-hover/tab:opacity-100"
      >
        <X size={10} />
      </span>
    </button>
  );
}

function TerminalPane({
  tab,
  visible,
}: {
  tab: TerminalTab;
  visible: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const terminalIdRef = useRef<string | null>(null);
  const [status, setStatus] = useState<"opening" | "ready" | "exited">("opening");
  const [error, setError] = useState<string | null>(null);

  const fontSize = useSettings((s) => s.terminalFontSize);
  const fontFamily = useSettings((s) => s.terminalFontFamily);
  const themeKey = useSettings((s) => s.terminalTheme);

  useEffect(() => {
    if (!containerRef.current) return;

    const {
      terminalFontSize,
      terminalFontFamily,
      terminalTheme,
      terminalScrollback,
    } = useSettings.getState();
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
        const id = await ipc.openTerminal(tab.sessionId, term.cols, term.rows);
        terminalIdRef.current = id;
        if (disposed) {
          ipc.closeTerminal(id).catch(() => {});
          return;
        }
        setStatus("ready");
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
    // Mount once per tab. tab.id changes only on new tabs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.id]);

  // Re-fit when this pane becomes visible — xterm can't measure a hidden DOM.
  useEffect(() => {
    if (!visible) return;
    const fit = fitRef.current;
    const term = termRef.current;
    if (!fit || !term) return;
    // Defer one frame so the layout has applied.
    const raf = requestAnimationFrame(() => {
      try {
        fit.fit();
      } catch {}
      term.focus();
    });
    return () => cancelAnimationFrame(raf);
  }, [visible]);

  // Live-apply font/theme changes without recreating the term.
  useEffect(() => {
    const term = termRef.current;
    const fit = fitRef.current;
    if (!term) return;
    term.options.fontSize = fontSize;
    term.options.fontFamily = fontFamily;
    term.options.theme = TERMINAL_THEMES[themeKey];
    if (fit && visible) {
      try {
        fit.fit();
      } catch {}
    }
  }, [fontSize, fontFamily, themeKey, visible]);

  const theme = TERMINAL_THEMES[themeKey];

  return (
    <div
      className={cn(
        "absolute inset-0 flex flex-col",
        visible ? "" : "pointer-events-none invisible"
      )}
      aria-hidden={!visible}
    >
      <div
        ref={containerRef}
        onMouseDown={() => termRef.current?.focus()}
        className="h-full w-full flex-1 overflow-hidden"
        style={{ background: theme.background }}
      />
      {status === "opening" && (
        <div className="pointer-events-none absolute right-2 top-2 rounded bg-bg-panel/70 px-1.5 py-0.5 text-[10px] text-text-dim">
          opening…
        </div>
      )}
      {error && (
        <div className="border-t border-border bg-danger-soft px-3 py-1 text-xs text-danger">
          {error}
        </div>
      )}
    </div>
  );
}

// Backwards-compatible re-export so any old import keeps working.
export const Terminal = TerminalDock;
