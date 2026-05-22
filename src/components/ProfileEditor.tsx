import { useState } from "react";
import { useConnections } from "@/stores/connectionsStore";
import { useSettings } from "@/stores/settingsStore";
import {
  PROTOCOL_DEFAULT_PORT,
  PROTOCOL_LABEL,
  type AuthMethod,
  type ConnectionProfile,
  type Protocol,
} from "@/lib/types";
import { ShieldCheck, ShieldOff, Terminal as TerminalIcon } from "lucide-react";

interface Props {
  profile: ConnectionProfile | null;
  onClose: () => void;
}

function genId(): string {
  return crypto.randomUUID();
}

export function ProfileEditor({ profile, onClose }: Props) {
  const saveProfile = useConnections((s) => s.saveProfile);
  const defaultPort = useSettings((s) => s.defaultPort);

  const [name, setName] = useState(profile?.name ?? "");
  const [protocol, setProtocol] = useState<Protocol>(profile?.protocol ?? "sftp");
  const [host, setHost] = useState(profile?.host ?? "");
  const [port, setPort] = useState(profile?.port ?? defaultPort);
  // Track whether the user has manually edited the port; if not, switching
  // protocol re-applies that protocol's standard port.
  const [portTouched, setPortTouched] = useState<boolean>(!!profile?.port);
  const [username, setUsername] = useState(profile?.username ?? "");
  const [authKind, setAuthKind] = useState<AuthMethod["kind"]>(
    profile?.auth.kind ?? "password"
  );
  const [password, setPassword] = useState(
    profile?.auth.kind === "password" ? profile.auth.password : ""
  );
  const [keyPath, setKeyPath] = useState(
    profile?.auth.kind === "key" ? profile.auth.path : ""
  );
  const [passphrase, setPassphrase] = useState(
    profile?.auth.kind === "key" ? profile.auth.passphrase ?? "" : ""
  );
  const [defaultRemotePath, setDefaultRemotePath] = useState(
    profile?.defaultRemotePath ?? "."
  );

  const onProtocolChange = (p: Protocol) => {
    setProtocol(p);
    if (!portTouched) {
      setPort(PROTOCOL_DEFAULT_PORT[p]);
    }
    // FTP / FTPS don't support key/agent auth — coerce to password.
    if ((p === "ftp" || p === "ftps") && authKind !== "password") {
      setAuthKind("password");
    }
  };

  const isFtp = protocol === "ftp" || protocol === "ftps";

  const save = async () => {
    let auth: AuthMethod;
    if (authKind === "password") {
      auth = { kind: "password", password };
    } else if (authKind === "key") {
      auth = { kind: "key", path: keyPath, passphrase: passphrase || undefined };
    } else {
      auth = { kind: "agent" };
    }

    const next: ConnectionProfile = {
      id: profile?.id ?? genId(),
      name: name || `${username}@${host}`,
      protocol,
      host,
      port,
      username,
      auth,
      defaultRemotePath: defaultRemotePath || undefined,
      color: profile?.color,
    };
    await saveProfile(next);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="anim-modal w-[30rem] rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3">
        <div className="mb-4 text-sm font-semibold">
          {profile ? "Edit connection" : "New connection"}
        </div>

        <Field label="Name">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="my-prod-box"
            className={inputCls}
          />
        </Field>

        <Field label="Protocol">
          <div className="grid grid-cols-3 gap-1 rounded-md border border-border bg-bg-subtle p-1">
            {(["sftp", "ftp", "ftps"] as Protocol[]).map((p) => (
              <ProtocolButton
                key={p}
                active={protocol === p}
                onClick={() => onProtocolChange(p)}
                label={PROTOCOL_LABEL[p]}
                hint={protocolHint(p)}
              />
            ))}
          </div>
        </Field>

        <div className="flex gap-2">
          <Field label="Host" className="flex-1">
            <input
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="example.com"
              className={inputCls}
            />
          </Field>
          <Field label="Port" className="w-24">
            <input
              type="number"
              value={port}
              onChange={(e) => {
                setPort(parseInt(e.target.value) || PROTOCOL_DEFAULT_PORT[protocol]);
                setPortTouched(true);
              }}
              className={inputCls}
            />
          </Field>
        </div>

        <Field label="Username">
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            placeholder={isFtp ? "anonymous" : "root"}
            className={inputCls}
          />
        </Field>

        <Field label="Auth">
          <select
            value={authKind}
            onChange={(e) => setAuthKind(e.target.value as AuthMethod["kind"])}
            className={inputCls}
          >
            <option value="password">Password</option>
            <option value="key" disabled={isFtp}>
              Private key file{isFtp ? " — SFTP only" : ""}
            </option>
            <option value="agent" disabled={isFtp}>
              SSH agent{isFtp ? " — SFTP only" : ""}
            </option>
          </select>
        </Field>

        {authKind === "agent" && (
          <Hint>
            Uses the running SSH agent for authentication.{" "}
            <span className="font-mono text-text-dim">
              {/* eslint-disable-next-line no-undef */}
              {navigator.platform.startsWith("Win")
                ? "\\\\.\\pipe\\openssh-ssh-agent (OpenSSH Authentication Agent service)"
                : "$SSH_AUTH_SOCK"}
            </span>
            . Run <span className="font-mono">ssh-add</span> first to load your keys.
          </Hint>
        )}

        {authKind === "password" && (
          <Field label="Password">
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              className={inputCls}
            />
          </Field>
        )}

        {authKind === "key" && (
          <>
            <Field label="Key path">
              <input
                value={keyPath}
                onChange={(e) => setKeyPath(e.target.value)}
                placeholder="~/.ssh/id_ed25519"
                className={inputCls}
              />
            </Field>
            <Field label="Passphrase (optional)">
              <input
                type="password"
                value={passphrase}
                onChange={(e) => setPassphrase(e.target.value)}
                className={inputCls}
              />
            </Field>
          </>
        )}

        <Field label="Default remote path">
          <input
            value={defaultRemotePath}
            onChange={(e) => setDefaultRemotePath(e.target.value)}
            placeholder={isFtp ? "/" : "."}
            className={inputCls}
          />
        </Field>

        {isFtp && (
          <Hint tone="warn">
            <ShieldOff size={11} className="inline-block mr-1 align-text-bottom" />
            FTP has no integrated terminal and{" "}
            {protocol === "ftp"
              ? "transmits credentials in clear text"
              : "uses TLS for the control channel only"}
            . Prefer SFTP when the server supports it.
          </Hint>
        )}

        <div className="mt-5 flex justify-end gap-2">
          <button
            className="rounded-md border border-border px-3.5 py-1.5 text-sm hover:bg-bg-hover"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            className="btn-accent rounded-md px-3.5 py-1.5 text-sm font-medium text-white disabled:opacity-40"
            onClick={save}
            disabled={!host || !username}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}

function protocolHint(p: Protocol): string {
  switch (p) {
    case "sftp":
      return "SSH · :22";
    case "ftp":
      return "Plain · :21";
    case "ftps":
      return "TLS · :21";
  }
}

function ProtocolButton({
  active,
  onClick,
  label,
  hint,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  hint: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={
        "flex flex-col items-start rounded-sm px-2 py-1.5 text-left transition-colors " +
        (active
          ? "bg-accent-soft text-text ring-1 ring-inset ring-accent/40"
          : "text-text-muted hover:bg-bg-hover hover:text-text")
      }
    >
      <span className="flex items-center gap-1 text-[11px] font-semibold">
        {label === "SFTP" ? (
          <TerminalIcon size={11} className={active ? "text-accent" : ""} />
        ) : label === "FTPS" ? (
          <ShieldCheck size={11} className={active ? "text-accent" : ""} />
        ) : (
          <ShieldOff size={11} />
        )}
        {label}
      </span>
      <span className="text-[10px] text-text-dim">{hint}</span>
    </button>
  );
}

function Hint({
  children,
  tone,
}: {
  children: React.ReactNode;
  tone?: "warn";
}) {
  return (
    <div
      className={
        "mb-3 rounded-md border px-2.5 py-2 text-[11.5px] leading-relaxed " +
        (tone === "warn"
          ? "border-danger/30 bg-danger-soft/60 text-text-muted"
          : "border-border bg-bg-subtle text-text-muted")
      }
    >
      {children}
    </div>
  );
}

const inputCls =
  "w-full rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-sm outline-none focus:border-accent";

function Field({
  label,
  className,
  children,
}: {
  label: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <label className={`mb-3 block ${className ?? ""}`}>
      <div className="mb-1 text-xs text-text-muted">{label}</div>
      {children}
    </label>
  );
}
