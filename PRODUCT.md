# Product

## Register

product

## Users

Developers, sysadmins, and DevOps engineers who manage remote servers and object storage daily. They live in terminals and IDEs, juggle many saved connections (SFTP/SSH, FTP/FTPS, S3, R2, B2, Azure Blob), and want one window instead of the FileZilla + PuTTY + cloud-console juggling act. Their context is focused work: moving files, syncing directories, editing remote files in place, running shell commands, and now lending a live session to an AI agent. They are keyboard-first, value density and speed, and trust a tool that gets out of the way.

## Product Purpose

Faro is a desktop client (Tauri + Rust + React) that unifies SFTP, FTP/FTPS, SSH terminals, and S3-compatible storage behind one connection list and one `RemoteFs` trait. Save a server once; browse it in a dual-pane file manager, open a terminal against the same SSH session, drag-and-drop transfers between panes, sync directories, and edit remote files in your local editor with auto-upload on save. The Agent Bridge lends an already-authenticated session to a local MCP/HTTP agent with per-command approval and zero shared credentials. Success: a power user replaces three legacy tools with Faro and never thinks about the UI, only the task.

## Brand Personality

Precise, dense, dev-native. Quietly confident, keyboard-first, information-rich without being noisy. The voice is plain and technical: say what the product literally does, never market it. It should feel like a power tool a sysadmin trusts at 2am, not a consumer app and not an enterprise dashboard. Emotional goal: control and trust, especially around security surfaces (host-key verification, Agent Bridge approvals) where the UI must make risk obvious.

## Anti-references

- **Dated FileZilla / PuTTY clutter** — the tools Faro replaces. No cramped gray toolbars, tiny inscrutable icons, or Win32 dialog soup.
- **Generic SaaS dashboard** — no cards-everywhere, hero-metric templates, pastel gradients, or marketing-flavored empty states. This is the AI-slop product look.
- **Consumer / playful app** — no blobby illustrations, big emoji, bouncy/elastic motion, or oversized friendly buttons. Density and restraint over cuteness.
- **Web-app-in-a-window** — it must read as a native desktop app: real density, OS-aware chrome (custom title bar, window controls), snappy interaction. Never a browser tab in disguise.

## Design Principles

1. **The tool disappears into the task.** Earned familiarity beats novelty. Standard affordances for standard jobs; surprise is reserved for moments, never for everyday controls.
2. **Density is a feature, legibility is non-negotiable.** Pack information for power users, but every label and value must clear contrast and hit a real touch/click target. Density never becomes illegibility.
3. **Make risk obvious.** Security surfaces (host-key mismatches, Agent Bridge command approvals, mirror-sync deletes) get danger-toned, unmissable UI. The user is always the gatekeeper and should always feel it.
4. **One vocabulary, every backend.** A button, form control, icon, or state means the same thing whether you're on SFTP, S3, or FTP. Capability differences hide affordances; they never reinvent them.
5. **State, not decoration.** Motion conveys state changes and feedback (150–250ms); color marks selection, action, and status. Neither exists for flourish.

## Accessibility & Inclusion

Target **WCAG 2.1 AAA where feasible** (7:1 body contrast), with AA as the hard floor. Known tension: a dense dark UI with muted/dim text tiers and an 11px status bar makes AAA hard in places; the audit flags where only AA is met so the tradeoff is deliberate, not accidental. Keyboard-first operation is a first-class requirement (command palette, shortcuts) — every interactive element needs a visible focus indicator and a reachable tab path. Respect `prefers-reduced-motion`. Themes must preserve contrast across all 7 variants, not just the default dark.
