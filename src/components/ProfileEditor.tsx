import { useState } from "react";
import { useConnections } from "@/stores/connectionsStore";
import { useSettings } from "@/stores/settingsStore";
import type { ConnectionProfile, AuthMethod } from "@/lib/types";

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
  const [host, setHost] = useState(profile?.host ?? "");
  const [port, setPort] = useState(profile?.port ?? defaultPort);
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

  const save = async () => {
    let auth: AuthMethod;
    if (authKind === "password") {
      auth = { kind: "password", password };
    } else if (authKind === "key") {
      auth = {
        kind: "key",
        path: keyPath,
        passphrase: passphrase || undefined,
      };
    } else {
      auth = { kind: "agent" };
    }

    const next: ConnectionProfile = {
      id: profile?.id ?? genId(),
      name: name || `${username}@${host}`,
      protocol: "sftp",
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
      <div className="anim-modal w-[28rem] rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3">
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
              onChange={(e) => setPort(parseInt(e.target.value) || 22)}
              className={inputCls}
            />
          </Field>
        </div>

        <Field label="Username">
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            placeholder="root"
            className={inputCls}
          />
        </Field>

        <Field label="Auth">
          <select
            value={authKind}
            onChange={(e) =>
              setAuthKind(e.target.value as AuthMethod["kind"])
            }
            className={inputCls}
          >
            <option value="password">Password</option>
            <option value="key">Private key file</option>
            <option value="agent" disabled>
              SSH agent (v0.2)
            </option>
          </select>
        </Field>

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
            placeholder="."
            className={inputCls}
          />
        </Field>

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
