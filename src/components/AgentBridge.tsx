import { useEffect, useState } from "react";
import {
  X,
  Copy,
  Check,
  Eye,
  EyeOff,
  Radio,
  ShieldCheck,
  Terminal as TerminalIcon,
  CircleAlert,
  CheckCircle2,
  Ban,
  Power,
} from "lucide-react";
import { useBridge } from "@/stores/bridgeStore";
import { useConnections } from "@/stores/connectionsStore";
import { useLayout } from "@/stores/layoutStore";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/format";
import type { BridgeApproval, ApprovalPolicy } from "@/lib/types";

// Phrase the approval prompt per operation kind.
const APPROVAL_COPY: Record<string, { title: string; foot: string }> = {
  exec: { title: "Agent wants to run a command", foot: "Runs over your authenticated SSH session." },
  read: { title: "Agent wants to read from the server", foot: "Reads through your authenticated Faro session." },
  download: { title: "Agent wants to download a file", foot: "Downloads through Faro's transfer engine." },
  upload: { title: "Agent wants to upload a file", foot: "Uploads through Faro's transfer engine." },
  search: { title: "Agent wants to search the server", foot: "Searches through your authenticated Faro session." },
};
function approvalCopy(kind: string) {
  return (
    APPROVAL_COPY[kind] ?? {
      title: "Agent wants to run an operation",
      foot: "Runs through your authenticated Faro session.",
    }
  );
}

// Always-mounted: boots the bridge store (status + event listeners) and renders
// the per-command approval modal whenever a request is pending.
export function AgentBridgeHost() {
  const approvals = useBridge((s) => s.approvals);
  const respond = useBridge((s) => s.respond);

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    let cancelled = false;
    useBridge
      .getState()
      .init()
      .then((c) => {
        if (cancelled) c();
        else cleanup = c;
      });
    return () => {
      cancelled = true;
      cleanup?.();
    };
  }, []);

  const pending = approvals[0];
  if (!pending) return null;
  return (
    <ApprovalModal
      approval={pending}
      count={approvals.length}
      onApprove={() => respond(pending.requestId, "approve")}
      onDeny={() => respond(pending.requestId, "deny")}
    />
  );
}

function ApprovalModal({
  approval,
  count,
  onApprove,
  onDeny,
}: {
  approval: BridgeApproval;
  count: number;
  onApprove: () => void;
  onDeny: () => void;
}) {
  const copy = approvalCopy(approval.kind);
  return (
    <div className="fixed inset-0 z-[85] flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="anim-modal w-[28rem] max-w-[92vw] rounded-xl border border-border bg-bg-panel shadow-elev-3">
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <ShieldCheck size={15} className="text-accent" />
          <span className="text-[15px] font-semibold tracking-tight">
            {copy.title}
          </span>
        </div>
        <div className="px-5 py-4">
          <div className="mb-2 text-xs text-text-muted">
            On <span className="font-medium text-text">{approval.sessionName}</span>{" "}
            via the Agent Bridge:
          </div>
          <pre className="selectable max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-border bg-bg-subtle px-3 py-2 font-mono text-xs text-text">
            {approval.command}
          </pre>
          <div className="mt-3 flex items-center gap-1.5 text-[11px] text-text-dim">
            <TerminalIcon size={11} /> {copy.foot}
          </div>
        </div>
        <div className="flex items-center justify-between gap-2 border-t border-border px-5 py-3">
          <span className="text-[11px] text-text-dim">
            {count > 1 ? `${count - 1} more queued` : ""}
          </span>
          <div className="flex gap-2">
            <button
              onClick={onDeny}
              className="flex items-center gap-1 rounded-md border border-border px-3 py-1.5 text-sm text-text-muted hover:bg-bg-hover hover:text-text"
            >
              <Ban size={13} /> Deny
            </button>
            <button
              onClick={onApprove}
              className="btn-accent flex items-center gap-1 rounded-md px-3.5 py-1.5 text-sm font-medium text-white"
            >
              <Check size={14} /> Approve &amp; run
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function skillMd(url: string, token: string, sessionId: string): string {
  return `---
name: faro-server
description: Operate on a remote server through Faro's authenticated session — run commands, browse, read, search and transfer files. Use when the user asks to inspect, run, or operate on a server they have open in Faro.
---

# Faro server access

Faro is bridging a live session at \`${url}\`. You can operate on the server
without any credentials — Faro holds the authenticated session and the user
approves requests in the Faro window (some kinds may be auto-approved per the
user's policy). Authenticate every call with the bearer token below.

All endpoints are \`POST\` with a JSON body and require these headers:
\`-H "Authorization: Bearer ${token}" -H "Content-Type: application/json"\`.

## List available sessions

\`\`\`bash
curl -s ${url}/sessions -H "Authorization: Bearer ${token}"
\`\`\`

## Run a command (SSH only)

\`\`\`bash
curl -s ${url}/exec -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" \\
  -d '{"sessionId":"${sessionId}","command":"df -h"}'
\`\`\`

Response: \`{ "stdout": "...", "stderr": "...", "exitCode": 0 }\`. Keep commands
non-interactive (no prompts/pagers); add flags like \`-y\`, \`| cat\`.

## Browse / read / search

\`\`\`bash
# list a directory
curl -s ${url}/list   -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}","path":"/var/log"}'
# read a text file (SSH/SFTP, capped at 256 KiB)
curl -s ${url}/read   -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}","path":"/etc/hostname"}'
# search by name (recursive, case-insensitive)
curl -s ${url}/search -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}","path":"/var/log","query":".log"}'
# session context
curl -s ${url}/info   -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}"}'
\`\`\`

## Transfer files

\`\`\`bash
# download to the user's machine (defaults to their Downloads folder)
curl -s ${url}/download -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}","path":"/etc/hosts"}'
# upload a local file to a remote directory
curl -s ${url}/upload   -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"sessionId":"${sessionId}","localPath":"/tmp/a.txt","remoteDir":"/home/user"}'
# check whether a transfer finished (poll until status is done/error)
curl -s ${url}/transfer -H "Authorization: Bearer ${token}" -H "Content-Type: application/json" -d '{"transferId":"<id from download/upload>"}'
\`\`\`

Notes:
- Requests block until the user approves them in Faro (or time out), unless the
  user has enabled auto-approve for that kind of operation.
- Transfers run in the background and also appear in Faro's transfer panel:
  \`/download\`/\`/upload\` return a \`transferId\`; poll \`/transfer\` to learn the
  outcome (status \`done\` or \`error\`).
- \`/exec\` output is capped (512 KiB) and times out after 60s; the result's
  \`truncated\`/\`timedOut\` flags tell you if it was cut short.
`;
}

function mcpAddCmd(url: string, token: string): string {
  return `claude mcp add --transport http faro ${url}/mcp --header "Authorization: Bearer ${token}"`;
}

// The Agent Bridge control panel (opened as a modal).
export function AgentBridge({ onClose }: { onClose: () => void }) {
  const status = useBridge((s) => s.status);
  const activity = useBridge((s) => s.activity);
  const start = useBridge((s) => s.start);
  const stop = useBridge((s) => s.stop);
  const setSessionAccess = useBridge((s) => s.setSessionAccess);
  const setPolicy = useBridge((s) => s.setPolicy);
  const refresh = useBridge((s) => s.refresh);
  const openDialog = useLayout((s) => s.openDialog);

  const activeSessionId = useConnections((s) => s.activeSessionId);
  const activeProfileId = useConnections((s) => s.activeProfileId);
  const profiles = useConnections((s) => s.profiles);
  const activeProfile = profiles.find((p) => p.id === activeProfileId);
  const isSsh = activeProfile?.protocol === "sftp";
  const granted =
    !!activeSessionId && status.enabledSessions.includes(activeSessionId);
  const policy = status.policy;
  const patchPolicy = (patch: Partial<ApprovalPolicy>) =>
    setPolicy({ ...policy, ...patch });

  const [showToken, setShowToken] = useState(false);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const canCopySetup = status.running && granted && !!activeSessionId;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        className="anim-modal flex max-h-[85vh] w-[38rem] max-w-[94vw] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <Radio size={15} className="text-accent" />
          <span className="text-[15px] font-semibold tracking-tight">
            Agent Bridge
          </span>
          {status.running && (
            <span className="flex items-center gap-1 rounded-full bg-emerald-500/15 px-1.5 py-0.5 text-[10px] font-medium text-emerald-400">
              <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" /> running
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={() => openDialog("agentConsole")}
            className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <TerminalIcon size={12} /> Live console
          </button>
          <button
            onClick={onClose}
            className="rounded-md p-1.5 text-text-muted hover:bg-bg-hover hover:text-text"
          >
            <X size={14} />
          </button>
        </div>

        <div className="flex-1 space-y-4 overflow-y-auto px-5 py-4">
          <p className="text-xs leading-relaxed text-text-muted">
            Let a local AI agent (Claude Code, Cursor…) operate on your connected
            servers through Faro — run commands, browse, read, search and transfer
            files, with no agent install on the server and no credentials shared.
            Faro brokers its authenticated session over a localhost endpoint and
            asks you to approve each request (unless you relax that below).
          </p>

          {/* Server */}
          <Card title="Local endpoint">
            <div className="flex items-center gap-2">
              <button
                onClick={status.running ? stop : start}
                className={cn(
                  "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium",
                  status.running
                    ? "border border-border text-text-muted hover:bg-bg-hover hover:text-text"
                    : "btn-accent text-white"
                )}
              >
                <Power size={13} />
                {status.running ? "Stop" : "Start bridge"}
              </button>
              {status.running && status.url && (
                <span className="font-mono text-xs text-text-muted">
                  {status.url}
                </span>
              )}
            </div>
            {status.running && (
              <div className="mt-3 space-y-2">
                <Row label="URL">
                  <span className="truncate font-mono text-xs">{status.url}</span>
                  <CopyButton text={status.url ?? ""} />
                </Row>
                <Row label="Token">
                  <span className="truncate font-mono text-xs">
                    {showToken
                      ? status.token
                      : "•".repeat(Math.min(24, status.token?.length ?? 0))}
                  </span>
                  <button
                    onClick={() => setShowToken((v) => !v)}
                    className="rounded p-0.5 text-text-dim hover:text-text"
                    title={showToken ? "Hide" : "Reveal"}
                  >
                    {showToken ? <EyeOff size={12} /> : <Eye size={12} />}
                  </button>
                  <CopyButton text={status.token ?? ""} />
                </Row>
              </div>
            )}
          </Card>

          {/* Access */}
          <Card title="Server access">
            {!activeSessionId ? (
              <div className="text-xs text-text-dim">
                Connect to a server first, then grant the agent access to it here.
              </div>
            ) : (
              <div className="flex items-center gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-sm">
                    Allow agent access —{" "}
                    <span className="font-medium">{activeProfile?.name}</span>
                  </div>
                  <div className="truncate font-mono text-[11px] text-text-dim">
                    {activeProfile?.username}@{activeProfile?.host}
                  </div>
                  {!isSsh && (
                    <div className="mt-0.5 text-[11px] text-text-dim">
                      File ops only (browse, read, search, transfer) — exec needs
                      an SSH session.
                    </div>
                  )}
                </div>
                <Toggle
                  checked={granted}
                  onChange={(v) => setSessionAccess(activeSessionId, v)}
                />
              </div>
            )}
          </Card>

          {/* Auto-approve */}
          <Card title="Auto-approve">
            <div className="mb-2.5 text-xs text-text-muted">
              By default Faro asks before every agent request. Loosen that here —
              applies to all enabled sessions.
            </div>
            <div className="space-y-2.5">
              <PolicyRow
                label="Allow all — no prompts"
                help="Approve every agent request (commands, reads, transfers) automatically. Most permissive."
                checked={policy.allowAll}
                onChange={(v) => patchPolicy({ allowAll: v })}
                danger
              />
              <PolicyRow
                label="Auto-approve read-only operations"
                help="List directories, read files and search run without asking. Downloads & uploads write to disk, so they still prompt unless Allow all is on."
                checked={policy.allowAll || policy.autoRead}
                disabled={policy.allowAll}
                onChange={(v) => patchPolicy({ autoRead: v })}
              />
              <PolicyRow
                label="Auto-approve safe shell commands"
                help="Read-only commands (ls, cat, df, grep…) run without asking; anything that could change the server still prompts. Best-effort heuristic."
                checked={policy.allowAll || policy.autoSafeExec}
                disabled={policy.allowAll}
                onChange={(v) => patchPolicy({ autoSafeExec: v })}
              />
            </div>
            {policy.allowAll && (
              <div className="mt-2.5 flex items-start gap-1.5 rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-1.5 text-[11px] text-amber-300/90">
                <CircleAlert size={12} className="mt-0.5 shrink-0" />
                The agent can run anything on enabled sessions without confirmation.
              </div>
            )}
          </Card>

          {/* Setup */}
          <Card title="Connect Claude Code">
            <div className="mb-2 text-xs text-text-muted">
              Native MCP — run this in your project and Claude Code gains{" "}
              <span className="font-mono">faro_exec</span>,{" "}
              <span className="font-mono">faro_list_dir</span>,{" "}
              <span className="font-mono">faro_read_file</span>,{" "}
              <span className="font-mono">faro_search</span>,{" "}
              <span className="font-mono">faro_download</span>/
              <span className="font-mono">upload</span> + more (approval follows
              your policy above):
            </div>
            <div className="flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded border border-border bg-bg-panel px-2 py-1 font-mono text-[10px] text-text-muted">
                {canCopySetup
                  ? mcpAddCmd(status.url ?? "", status.token ?? "")
                  : "claude mcp add --transport http faro …"}
              </code>
              <CopyButton
                disabled={!canCopySetup}
                text={
                  canCopySetup ? mcpAddCmd(status.url ?? "", status.token ?? "") : ""
                }
              />
            </div>
            <div className="mt-3 flex items-center gap-2 text-[11px] text-text-dim">
              <span>Prefer a curl-based skill?</span>
              <CopyButton
                disabled={!canCopySetup}
                label="Copy SKILL.md"
                text={
                  canCopySetup
                    ? skillMd(
                        status.url ?? "",
                        status.token ?? "",
                        activeSessionId ?? ""
                      )
                    : ""
                }
              />
            </div>
            {!canCopySetup && (
              <div className="mt-1.5 text-[11px] text-text-dim">
                Start the bridge and grant access to a session to enable this.
              </div>
            )}
          </Card>

          {/* Activity */}
          <Card title="Activity">
            {activity.length === 0 ? (
              <div className="text-xs text-text-dim">
                No agent activity yet. Approved commands and denials show up here.
              </div>
            ) : (
              <div className="max-h-48 space-y-1 overflow-y-auto">
                {activity.slice(0, 60).map((a) => (
                  <div key={a.id} className="flex items-start gap-2 text-[11px]">
                    <ActivityIcon kind={a.kind} ok={a.ok} />
                    <span className="min-w-0 flex-1 truncate font-mono text-text-muted">
                      {a.detail}
                    </span>
                    <span className="shrink-0 text-text-dim">{relTime(a.at)}</span>
                  </div>
                ))}
              </div>
            )}
          </Card>
        </div>

        <div className="border-t border-border px-5 py-3 text-[11px] text-text-dim">
          <ShieldCheck size={11} className="mr-1 inline" />
          Bound to 127.0.0.1 only · token required · per-session opt-in ·{" "}
          {policy.allowAll
            ? "auto-approving all requests"
            : policy.autoRead || policy.autoSafeExec
              ? "auto-approving some requests"
              : "you approve every request"}
          .
        </div>
      </div>
    </div>
  );
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border-subtle bg-bg-subtle/40 p-3">
      <div className="mb-2 text-[11px] font-semibold uppercase tracking-[0.08em] text-text-muted">
        {title}
      </div>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-12 shrink-0 text-[11px] text-text-dim">{label}</span>
      <div className="flex min-w-0 flex-1 items-center gap-2 rounded border border-border bg-bg-panel px-2 py-1">
        {children}
      </div>
    </div>
  );
}

function CopyButton({
  text,
  label,
  wide,
  disabled,
}: {
  text: string;
  label?: string;
  wide?: boolean;
  disabled?: boolean;
}) {
  const [done, setDone] = useState(false);
  return (
    <button
      disabled={disabled}
      onClick={() => {
        navigator.clipboard.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1200);
      }}
      className={cn(
        "flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[10px] text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-40",
        wide && "px-2.5 py-1.5 text-xs"
      )}
    >
      {done ? <Check size={11} /> : <Copy size={11} />}
      {label ?? (done ? "Copied" : "Copy")}
    </button>
  );
}

function Toggle({
  checked,
  onChange,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-5 w-9 shrink-0 rounded-full border transition-colors",
        disabled && "cursor-not-allowed opacity-50",
        checked
          ? "border-accent bg-accent"
          : "border-border bg-bg-subtle hover:border-text-dim"
      )}
    >
      <span
        className={cn(
          "absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white shadow-elev-1 transition-transform",
          checked ? "translate-x-4" : "translate-x-0"
        )}
      />
    </button>
  );
}

function PolicyRow({
  label,
  help,
  checked,
  onChange,
  disabled,
  danger,
}: {
  label: string;
  help: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  danger?: boolean;
}) {
  return (
    <div className="flex items-start gap-3">
      <div className="min-w-0 flex-1">
        <div className={cn("text-sm", danger && "font-medium text-amber-300")}>
          {label}
        </div>
        <div className="text-[11px] leading-snug text-text-dim">{help}</div>
      </div>
      <Toggle checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}

function ActivityIcon({ kind, ok }: { kind: string; ok: boolean }) {
  if (kind === "denied")
    return <Ban size={12} className="mt-0.5 shrink-0 text-text-dim" />;
  if (kind === "error" || !ok)
    return <CircleAlert size={12} className="mt-0.5 shrink-0 text-danger" />;
  return <CheckCircle2 size={12} className="mt-0.5 shrink-0 text-emerald-400" />;
}
