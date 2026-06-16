import { useEffect, useId, useRef, useState } from "react";
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
  Plus,
  Pencil,
  Trash2,
  RefreshCw,
  Play,
} from "lucide-react";
import { useBridge } from "@/stores/bridgeStore";
import { useConnections } from "@/stores/connectionsStore";
import { useLayout } from "@/stores/layoutStore";
import { useDialog } from "@/hooks/useDialog";
import { ipc } from "@/lib/ipc";
import { toast } from "@/stores/toastStore";
import { ConfirmModal } from "./ConfirmModal";
import { cn } from "@/lib/cn";
import { relTime } from "@/lib/format";
import type {
  BridgeApproval,
  ApprovalPolicy,
  BridgeActivity,
  SavedCommand,
} from "@/lib/types";

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
  const panelRef = useRef<HTMLDivElement>(null);
  const denyRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  // Escape denies (the safe choice); Deny takes initial focus so a stray
  // keypress never auto-approves an agent command.
  useDialog(panelRef, { onClose: onDeny, initialFocus: denyRef });
  return (
    <div className="fixed inset-0 z-secure flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="anim-modal w-[28rem] max-w-[92vw] rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <ShieldCheck size={15} className="text-accent" />
          <span id={titleId} className="text-[15px] font-semibold tracking-tight">
            {copy.title}
          </span>
        </div>
        <div className="px-5 py-4">
          <div className="mb-2 text-xs text-text-muted">
            On <span className="font-medium text-text">{approval.sessionName}</span>{" "}
            via the Agent Bridge:
          </div>
          <pre className="selectable max-h-40 max-w-full overflow-auto whitespace-pre-wrap [overflow-wrap:anywhere] rounded-md border border-border bg-bg-subtle px-3 py-2 font-mono text-xs text-text">
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
              ref={denyRef}
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

function savedCommandsBlock(server: string, commands: SavedCommand[]): string {
  const S = server || "<server>";
  if (commands.length === 0) {
    return `## Saved commands

The user can save named, pre-approved commands in Faro's Agent Bridge panel; once
they do, run them with \`faro-cli agent run "<name>" ${S}\` (no approval prompt).
Check what's available any time with \`faro-cli agent commands\`.

`;
  }
  return `## Saved commands (prefer these)

The user has saved these named, **pre-approved** commands. Prefer running one by
name over composing a raw command — it runs immediately (no approval prompt) and
exactly as the user vetted it. You only supply the name and the connection:

\`\`\`bash
faro-cli agent commands                          # list saved commands
faro-cli agent run "<name>" ${S}                 # run one by name
\`\`\`

Available:
${commands
  .map(
    (c) =>
      `- \`${c.name}\` — \`${c.command}\`${c.description ? `  (${c.description})` : ""}`
  )
  .join("\n")}

`;
}

function skillMdReadOnly(server: string, commands: SavedCommand[]): string {
  const S = server || "<server>";
  const savedBlock = savedCommandsBlock(S, commands);
  return `---
name: faro-helper
description: Help the user with their own Faro file client — browse, read, search and manage files on computers and servers they are already connected to. Use only for the routine file-management and read-only diagnostic tasks the user asks for.
---

# Helping the user in Faro (read-only)

Faro is the user's desktop SSH/SFTP file client (FileZilla + PuTTY). They have
already opened and authenticated a connection to a computer or server **they own
or operate**, and they have turned on the local Agent Bridge so you can help them
through it. Nothing runs unless the user first enables access and, by default,
approves each action in the Faro UI.

You help through **\`faro-cli agent\`**, which talks to Faro's local Agent Bridge.
You never handle a URL, port or token — \`faro-cli\` reads them from Faro's local
discovery file. You only need the **connection name** (here: \`${S}\`).

You are using the **read-only helper** skill. You can browse, read, search and
transfer files, but you do **not** run arbitrary commands. If the user asks for
something that needs a command (for example checking disk usage or service
status), tell them to switch to the admin skill or run it themselves.

## Before you start

Make sure Faro is running with the **Agent Bridge turned on** (the master switch
at the top of the Bridge panel) and that this connection has been granted access.
Confirm with:

\`\`\`bash
faro-cli agent context           # bridge state, sessions, saved commands
faro-cli agent sessions          # connections you may help with
\`\`\`

If it reports the bridge isn't running, ask the user to turn it on. (\`faro-cli\`
must be on your PATH; if it isn't, the user can give you its full path.)

${savedBlock}## Browse / read / search

Use these for everyday file and diagnostics work:

\`\`\`bash
faro-cli agent ls     ${S} /var/log
faro-cli agent read   ${S} /etc/hostname       # SSH/SFTP, capped at 256 KiB
faro-cli agent search ${S} ".log" /var/log     # search <server> <query> [path]
faro-cli agent info   ${S}                      # protocol, host, port, …
\`\`\`

## Transfer files

\`\`\`bash
faro-cli agent download ${S} /etc/hosts         # → the user's Downloads folder
faro-cli agent upload   ${S} ./a.txt /home/user # local file → remote dir
faro-cli agent transfer <transferId>            # poll until done/error
\`\`\`

## Review what you've done

\`\`\`bash
faro-cli agent history --server ${S}            # recent activity, newest first
\`\`\`

Notes:
- Only help with tasks the user explicitly asks for. When in doubt, ask first.
- Each call may pause while the user approves it in Faro (unless they pre-approved
  that kind). A blocked call means they're being asked to confirm.
- Transfers run in the background and also appear in Faro's transfer panel; poll
  \`agent transfer <id>\` for the outcome.
`;
}

function skillMdAdmin(server: string, commands: SavedCommand[]): string {
  const S = server || "<server>";
  const savedBlock = savedCommandsBlock(S, commands);
  return `---
name: faro-helper-admin
description: Help the user manage their own Faro file client and connected servers — browse, read, search, transfer files and run status/diagnostic commands on computers and servers they already connected to. Use only for the routine administration and debugging tasks the user asks for.
---

# Helping the user in Faro (admin)

Faro is the user's desktop SSH/SFTP file client (FileZilla + PuTTY). They have
already opened and authenticated a connection to a computer or server **they own
or operate**, and they have turned on the local Agent Bridge so you can help them
through it. Nothing runs unless the user first enables access and, by default,
approves each action in the Faro UI.

You help through **\`faro-cli agent\`**, which talks to Faro's local Agent Bridge.
You never handle a URL, port or token — \`faro-cli\` reads them from Faro's local
discovery file. You only need the **connection name** (here: \`${S}\`).

You are using the **admin helper** skill, which can also run commands on connected
SSH servers. Prefer read-only inspection first, and only run commands the user
explicitly asks for.

## Before you start

Make sure Faro is running with the **Agent Bridge turned on** (the master switch
at the top of the Bridge panel) and that this connection has been granted access.
Confirm with:

\`\`\`bash
faro-cli agent context           # bridge state, sessions, saved commands
faro-cli agent sessions          # connections you may help with
\`\`\`

If it reports the bridge isn't running, ask the user to turn it on. (\`faro-cli\`
must be on your PATH; if it isn't, the user can give you its full path.)

${savedBlock}## Browse / read / search

Use these for everyday file and diagnostics work:

\`\`\`bash
faro-cli agent ls     ${S} /var/log
faro-cli agent read   ${S} /etc/hostname       # SSH/SFTP, capped at 256 KiB
faro-cli agent search ${S} ".log" /var/log     # search <server> <query> [path]
faro-cli agent info   ${S}                      # protocol, host, port, …
\`\`\`

## Run a status or diagnostic command (SSH only)

For checking state only — for example disk usage, service status or log tailing:

\`\`\`bash
faro-cli agent exec ${S} "df -h"
\`\`\`

Keep commands non-interactive (no prompts/pagers); add \`-y\`, \`| cat\`, etc. The
remote exit code is propagated; stderr is printed to stderr. Avoid mutating
commands unless the user explicitly asks for them.

## Transfer files

\`\`\`bash
faro-cli agent download ${S} /etc/hosts         # → the user's Downloads folder
faro-cli agent upload   ${S} ./a.txt /home/user # local file → remote dir
faro-cli agent transfer <transferId>            # poll until done/error
\`\`\`

## Review what you've done

\`\`\`bash
faro-cli agent history --server ${S}            # recent activity, newest first
\`\`\`

Notes:
- Only help with tasks the user explicitly asks for. When in doubt, ask first.
- Each call may pause while the user approves it in Faro (unless they pre-approved
  that kind). A blocked call means they're being asked to confirm.
- Transfers run in the background and also appear in Faro's transfer panel; poll
  \`agent transfer <id>\` for the outcome.
- \`exec\` output is capped (512 KiB) and times out after 60s.
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
  const setEnabled = useBridge((s) => s.setEnabled);
  const setSessionAccess = useBridge((s) => s.setSessionAccess);
  const setPolicy = useBridge((s) => s.setPolicy);
  const refresh = useBridge((s) => s.refresh);
  const savedCommands = useBridge((s) => s.savedCommands);
  const saveCommand = useBridge((s) => s.saveCommand);
  const deleteCommand = useBridge((s) => s.deleteCommand);
  const loadCommands = useBridge((s) => s.loadCommands);
  const refreshActivity = useBridge((s) => s.refreshActivity);
  const clearActivity = useBridge((s) => s.clearActivity);
  const setConsoleOpen = useLayout((s) => s.setConsoleOpen);

  const activeSessionId = useConnections((s) => s.activeSessionId);
  const sessions = useConnections((s) => s.sessions);
  const profiles = useConnections((s) => s.profiles);
  // Every live connection, paired with its profile, for per-session toggles.
  const liveSessions = sessions
    .map((s) => ({ ...s, profile: profiles.find((p) => p.id === s.profileId) }))
    .filter((s) => s.profile);
  const anyGranted = status.enabledSessions.length > 0;
  // The session the copy-paste setup snippet targets: prefer the focused one if
  // it's enabled, otherwise the first enabled session.
  const setupSessionId =
    activeSessionId && status.enabledSessions.includes(activeSessionId)
      ? activeSessionId
      : status.enabledSessions[0] ?? null;
  // The CLI matches servers by their profile NAME, so the skill snippet uses
  // the name (not the session UUID) of the session we're targeting.
  const setupServerName =
    liveSessions.find((s) => s.sessionId === setupSessionId)?.profile?.name ??
    null;
  const policy = status.policy;
  const patchPolicy = (patch: Partial<ApprovalPolicy>) =>
    setPolicy({ ...policy, ...patch });

  const [showToken, setShowToken] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });

  useEffect(() => {
    refresh();
    loadCommands();
  }, [refresh, loadCommands]);

  // Resolve a session id -> profile name + color for the history rows.
  const sessionMeta = new Map(
    liveSessions.map((s) => [
      s.sessionId,
      { name: s.profile?.name ?? "", color: s.profile?.color },
    ])
  );

  const canCopySetup =
    status.enabled && status.running && anyGranted && !!setupServerName;

  return (
    <div
      className="fixed inset-0 z-modal flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
        className="anim-modal flex max-h-[85vh] w-[38rem] max-w-[94vw] flex-col rounded-xl border border-border bg-bg-panel shadow-elev-3"
      >
        <div className="flex items-center gap-2 border-b border-border px-5 py-3.5">
          <Radio size={15} className="text-accent" />
          <span id={titleId} className="text-[15px] font-semibold tracking-tight">
            Agent Bridge
          </span>
          {status.running && (
            <span className="flex items-center gap-1 rounded-full bg-success/15 px-1.5 py-0.5 text-[10px] font-medium text-success">
              <span className="h-1.5 w-1.5 rounded-full bg-success" /> running
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={() => {
              setConsoleOpen(true);
              onClose();
            }}
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
          {/* Master switch — default off; off means nothing is exposed. */}
          <Card title="Agent Bridge">
            <div className="flex items-start gap-3">
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium">
                  {status.enabled
                    ? "On — AI access available"
                    : "Off — nothing exposed"}
                </div>
                <div className="text-[11px] leading-snug text-text-dim">
                  Off means no token and no localhost server exist — nothing is
                  exposed. Turn it on only when you want AI help; even then, each
                  connection stays private until you grant it access below.
                </div>
              </div>
              <Toggle checked={status.enabled} onChange={(v) => setEnabled(v)} />
            </div>
          </Card>

          <p className="text-xs leading-relaxed text-text-muted">
            Let a local AI agent (Claude Code, Cursor…) operate on your connected
            servers through Faro — run commands, browse, read, search and transfer
            files, with no agent install on the server and no credentials shared.
            Faro brokers its authenticated session over a localhost endpoint and
            asks you to approve each request (unless you relax that below).
          </p>

          <div
            className={cn(
              "space-y-4",
              !status.enabled && "pointer-events-none select-none opacity-50"
            )}
            aria-disabled={!status.enabled}
          >
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
            {liveSessions.length === 0 ? (
              <div className="text-xs text-text-dim">
                Connect to a server first, then grant the agent access to it here.
                Each connection gets its own toggle — bridge as many at once as
                you like.
              </div>
            ) : (
              <div className="space-y-1.5">
                {liveSessions.map(({ sessionId, profile }) => {
                  const p = profile!;
                  const isSsh = p.protocol === "sftp";
                  const isObject = p.protocol === "s3" || p.protocol === "azure";
                  return (
                    <div
                      key={sessionId}
                      className="flex items-center gap-3 rounded-md bg-bg/60 px-2.5 py-2"
                    >
                      <span
                        className="h-2.5 w-2.5 shrink-0 rounded-full"
                        style={{ background: p.color || "rgb(var(--accent))" }}
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5 text-sm">
                          <span className="truncate font-medium">{p.name}</span>
                          {sessionId === activeSessionId && (
                            <span className="shrink-0 rounded-sm bg-accent-soft px-1 text-[9px] font-medium uppercase tracking-wider text-accent">
                              active
                            </span>
                          )}
                        </div>
                        <div className="truncate font-mono text-[11px] text-text-dim">
                          {isObject
                            ? `${p.protocol}://${p.bucket ?? "?"}`
                            : `${p.username}@${p.host}`}
                        </div>
                        {!isSsh && (
                          <div className="mt-0.5 text-[11px] text-text-dim">
                            File ops only — exec needs an SSH session.
                          </div>
                        )}
                      </div>
                      <Toggle
                        checked={status.enabledSessions.includes(sessionId)}
                        onChange={(v) => setSessionAccess(sessionId, v)}
                      />
                    </div>
                  );
                })}
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
              <div className="mt-2.5 flex items-start gap-1.5 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-1.5 text-[11px] text-warning">
                <CircleAlert size={12} className="mt-0.5 shrink-0" />
                The agent can run anything on enabled sessions without confirmation.
              </div>
            )}
          </Card>

          {/* Setup — MCP is the recommended path; skills are the fallback. */}
          <Card title="Connect your AI agent">
            <div className="mb-2 text-xs text-text-muted">
              <span className="font-medium text-text">Recommended.</span> Add Faro
              as an MCP server in Claude Code, Cursor or any MCP-compatible agent.
              The agent talks directly to Faro over a local endpoint — no big skill
              text to paste, and it auto-discovers the tools.
            </div>
            <div className="flex items-center gap-2">
              <code className="min-w-0 flex-1 truncate rounded bg-bg px-2 py-1.5 font-mono text-[10px] text-text-muted">
                {canCopySetup
                  ? mcpAddCmd(status.url ?? "", status.token ?? "")
                  : "claude mcp add --transport http faro …"}
              </code>
              <CopyButton
                disabled={!canCopySetup}
                text={
                  canCopySetup
                    ? mcpAddCmd(status.url ?? "", status.token ?? "")
                    : ""
                }
              />
            </div>
            <button
              disabled={!canCopySetup}
              onClick={async () => {
                try {
                  const msg = await ipc.bridgeRegisterMcp(
                    status.url ?? "",
                    status.token ?? ""
                  );
                  toast.success("MCP registered", msg);
                } catch (e) {
                  toast.error("Couldn't register MCP", String(e));
                }
              }}
              className="mt-2 flex w-full items-center justify-center gap-1 rounded-md border border-border bg-bg px-2 py-1.5 text-[11px] font-medium text-text hover:bg-bg-hover disabled:opacity-40"
            >
              <Plus size={12} /> Add to Claude Code automatically
            </button>

            <details className="mt-4 text-[11px] text-text-dim">
              <summary className="cursor-pointer select-none hover:text-text">
                Fallback: paste a skill (no MCP support)
              </summary>
              <div className="mt-2 space-y-2">
                <div className="text-xs text-text-muted">
                  If your agent doesn't support MCP, paste one of these skills
                  instead. The read-only skill is safest and least likely to be
                  flagged; the admin skill also allows diagnostic commands on SSH
                  servers.
                </div>
                <div className="flex flex-col gap-2 sm:flex-row">
                  <CopyButton
                    wide
                    disabled={!canCopySetup}
                    label="Copy read-only skill"
                    text={
                      canCopySetup
                        ? skillMdReadOnly(setupServerName ?? "", savedCommands)
                        : ""
                    }
                  />
                  <CopyButton
                    wide
                    disabled={!canCopySetup}
                    label="Copy admin skill"
                    text={
                      canCopySetup
                        ? skillMdAdmin(setupServerName ?? "", savedCommands)
                        : ""
                    }
                  />
                </div>
              </div>
            </details>
            {!canCopySetup && (
              <div className="mt-1.5 text-[11px] text-text-dim">
                Start the bridge and grant access to a session to enable this.
              </div>
            )}
          </Card>

          {/* Saved commands — pre-approved, run by name. */}
          <SavedCommandsCard
            commands={savedCommands}
            onSave={saveCommand}
            onDelete={deleteCommand}
          />

          {/* History — what the agent has done (in-memory, last 200). */}
          <HistoryCard
            activity={activity}
            sessionMeta={sessionMeta}
            onRefresh={refreshActivity}
            onClear={clearActivity}
          />
          </div>
        </div>

        <div className="border-t border-border px-5 py-3 text-[11px] text-text-dim">
          <ShieldCheck size={11} className="mr-1 inline" />
          {!status.enabled ? (
            "Agent Bridge is off — no token or localhost server exists."
          ) : (
            <>
              Bound to 127.0.0.1 only · token required · per-session opt-in ·{" "}
              {policy.allowAll
                ? "auto-approving all requests"
                : policy.autoRead || policy.autoSafeExec
                  ? "auto-approving some requests"
                  : "you approve every request"}
              .
            </>
          )}
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
      <div className="flex min-w-0 flex-1 items-center gap-2 rounded bg-bg px-2 py-1">
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
        <div className={cn("text-sm", danger && "font-medium text-warning")}>
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
  return <CheckCircle2 size={12} className="mt-0.5 shrink-0 text-success" />;
}

function SavedCommandsCard({
  commands,
  onSave,
  onDelete,
}: {
  commands: SavedCommand[];
  onSave: (c: SavedCommand) => void;
  onDelete: (id: string) => void;
}) {
  const [editing, setEditing] = useState<SavedCommand | "new" | null>(null);
  const [deleting, setDeleting] = useState<SavedCommand | null>(null);
  return (
    <Card title="Saved commands">
      <div className="mb-2 flex items-start gap-2">
        <div className="min-w-0 flex-1 text-[11px] leading-snug text-text-dim">
          Named, <span className="text-warning">pre-approved</span> commands the
          agent runs by name with no prompt — it can't change the command, only run
          it. (<span className="font-mono">faro-cli agent run "&lt;name&gt;"</span>)
        </div>
        <button
          onClick={() => setEditing("new")}
          className="flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-text-muted hover:bg-bg-hover hover:text-text"
        >
          <Plus size={12} /> Add
        </button>
      </div>
      {commands.length === 0 ? (
        <div className="text-xs text-text-dim">
          No saved commands yet. Add one the agent can run by name.
        </div>
      ) : (
        <div className="space-y-1">
          {commands.map((c) => (
            <div
              key={c.id}
              className="group flex items-center gap-2 rounded-md bg-bg px-2 py-1.5"
            >
              <Play size={12} className="shrink-0 text-success" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-[12px] font-medium">{c.name}</span>
                  {c.description && (
                    <span className="truncate text-[10px] text-text-dim">
                      {c.description}
                    </span>
                  )}
                </div>
                <div className="truncate font-mono text-[10px] text-text-muted">
                  {c.command}
                </div>
              </div>
              <button
                onClick={() => setEditing(c)}
                title="Edit"
                className="shrink-0 rounded p-1 text-text-dim opacity-0 hover:bg-bg-hover hover:text-text group-hover:opacity-100"
              >
                <Pencil size={12} />
              </button>
              <button
                onClick={() => setDeleting(c)}
                title="Delete"
                className="shrink-0 rounded p-1 text-text-dim opacity-0 hover:bg-bg-hover hover:text-danger group-hover:opacity-100"
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
      {editing && (
        <SavedCommandEditor
          command={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSave={(c) => {
            onSave(c);
            setEditing(null);
          }}
        />
      )}
      {deleting && (
        <ConfirmModal
          title={`Delete "${deleting.name}"?`}
          message="The agent will no longer be able to run this command by name."
          destructive
          confirmLabel="Delete"
          onClose={() => setDeleting(null)}
          onConfirm={() => {
            onDelete(deleting.id);
            setDeleting(null);
          }}
        />
      )}
    </Card>
  );
}

function SavedCommandEditor({
  command,
  onClose,
  onSave,
}: {
  command: SavedCommand | null;
  onClose: () => void;
  onSave: (c: SavedCommand) => void;
}) {
  const [name, setName] = useState(command?.name ?? "");
  const [cmd, setCmd] = useState(command?.command ?? "");
  const [description, setDescription] = useState(command?.description ?? "");
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });
  const canSave = name.trim() !== "" && cmd.trim() !== "";
  const submit = () => {
    if (!canSave) return;
    onSave({
      id: command?.id ?? "",
      name: name.trim(),
      command: cmd.trim(),
      description: description.trim(),
    });
  };
  const inputCls =
    "w-full rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent";
  return (
    <div
      className="fixed inset-0 z-palette flex items-center justify-center bg-black/60 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={(e) => e.stopPropagation()}
        className="anim-modal w-[30rem] max-w-[92vw] rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3"
      >
        <div id={titleId} className="mb-3 text-sm font-semibold">
          {command ? "Edit saved command" : "New saved command"}
        </div>
        <label className="mb-3 block">
          <div className="mb-1 text-xs text-text-muted">Name</div>
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="restart-web"
            className={inputCls}
          />
        </label>
        <label className="mb-3 block">
          <div className="mb-1 text-xs text-text-muted">Command</div>
          <textarea
            value={cmd}
            onChange={(e) => setCmd(e.target.value)}
            placeholder="sudo systemctl restart nginx"
            rows={3}
            className={cn(inputCls, "resize-y font-mono text-[13px]")}
          />
        </label>
        <label className="mb-2 block">
          <div className="mb-1 text-xs text-text-muted">Description (optional)</div>
          <input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Restart the web server"
            className={inputCls}
          />
        </label>
        <div className="mb-4 flex items-start gap-1.5 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-1.5 text-[11px] text-warning">
          <CircleAlert size={12} className="mt-0.5 shrink-0" />
          The agent runs this exact command with no approval prompt.
        </div>
        <div className="flex items-center justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-md border border-border px-3.5 py-1.5 text-sm hover:bg-bg-hover"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={!canSave}
            className="btn-accent rounded-md px-3.5 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-40"
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}

function HistoryCard({
  activity,
  sessionMeta,
  onRefresh,
  onClear,
}: {
  activity: BridgeActivity[];
  sessionMeta: Map<string, { name: string; color?: string }>;
  onRefresh: () => void;
  onClear: () => void;
}) {
  return (
    <Card title="History">
      <div className="mb-2 flex items-center gap-2">
        <div className="min-w-0 flex-1 text-[11px] text-text-dim">
          What the agent has done — newest first (kept in memory, last 200).
        </div>
        <button
          onClick={onRefresh}
          title="Refresh"
          className="shrink-0 rounded p-1 text-text-dim hover:bg-bg-hover hover:text-text"
        >
          <RefreshCw size={12} />
        </button>
        <button
          onClick={onClear}
          disabled={activity.length === 0}
          title="Clear history"
          className="shrink-0 rounded p-1 text-text-dim hover:bg-bg-hover hover:text-danger disabled:opacity-40"
        >
          <Trash2 size={12} />
        </button>
      </div>
      {activity.length === 0 ? (
        <div className="text-xs text-text-dim">
          Nothing yet. Approved commands, reads, transfers and denials show up here.
        </div>
      ) : (
        <div className="max-h-48 space-y-1 overflow-y-auto">
          {activity.slice(0, 100).map((a) => {
            const meta = sessionMeta.get(a.sessionId);
            return (
              <div key={a.id} className="flex items-start gap-2 text-[11px]">
                <ActivityIcon kind={a.kind} ok={a.ok} />
                {meta && meta.name && (
                  <span className="flex shrink-0 items-center gap-1 text-text-dim">
                    <span
                      className="h-1.5 w-1.5 rounded-full"
                      style={{ background: meta.color || "rgb(var(--accent))" }}
                    />
                    <span className="max-w-[6rem] truncate">{meta.name}</span>
                  </span>
                )}
                <span className="min-w-0 flex-1 truncate font-mono text-text-muted">
                  {a.detail}
                </span>
                <span className="shrink-0 text-text-dim">{relTime(a.at)}</span>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}
