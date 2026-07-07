import { useCallback, useEffect, useId, useRef, useState } from "react";
import { useConnections } from "@/stores/connectionsStore";
import { useSettings } from "@/stores/settingsStore";
import { useDialog } from "@/hooks/useDialog";
import {
  PROTOCOL_DEFAULT_PORT,
  PROTOCOL_LABEL,
  S3_PROVIDER_PRESETS,
  isObjectProtocol,
  isAgentProtocol,
  type AuthMethod,
  type ConnectionProfile,
  type Protocol,
  type S3Provider,
  type DiscoveredAgent,
} from "@/lib/types";
import {
  ShieldCheck,
  ShieldOff,
  Terminal as TerminalIcon,
  Cloud,
  Eye,
  EyeOff,
  Wand2,
  Check,
  MonitorSmartphone,
  Radar,
  Loader2,
  Link2,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { generatePassword } from "@/lib/password";
import { ipc } from "@/lib/ipc";
import { toast } from "@/stores/toastStore";

interface Props {
  profile: ConnectionProfile | null;
  onClose: () => void;
}

function genId(): string {
  return crypto.randomUUID();
}

/// Guess which provider a saved S3 profile belongs to from its endpoint URL.
/// We never need this to be perfect — it just preselects a preset button.
function guessProvider(endpoint?: string): S3Provider {
  if (!endpoint) return "aws";
  const e = endpoint.toLowerCase();
  if (e.includes("r2.cloudflarestorage.com")) return "r2";
  if (e.includes("backblazeb2.com")) return "b2";
  return "aws";
}

export function ProfileEditor({ profile, onClose }: Props) {
  const saveProfile = useConnections((s) => s.saveProfile);
  const connectProfile = useConnections((s) => s.connect);
  const defaultPort = useSettings((s) => s.defaultPort);

  // Stable id for the lifetime of the editor, so pairing (which persists to this
  // id) and a later Save target the same profile.
  const [id] = useState(profile?.id ?? genId());
  const [name, setName] = useState(profile?.name ?? "");
  const [protocol, setProtocol] = useState<Protocol>(profile?.protocol ?? "sftp");
  // Faro Agent: the pinned daemon key. Set by pairing; carried through Save.
  const [agentKey, setAgentKey] = useState<string | undefined>(profile?.agentKey);
  const [host, setHost] = useState(profile?.host ?? "");
  const [port, setPort] = useState(profile?.port ?? defaultPort);
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
  const [autoConnect, setAutoConnect] = useState(profile?.autoConnect ?? false);

  // S3-only state.
  const [bucket, setBucket] = useState(profile?.bucket ?? "");
  const [region, setRegion] = useState(profile?.region ?? "us-east-1");
  const [endpoint, setEndpoint] = useState(profile?.endpoint ?? "");
  const [s3Provider, setS3Provider] = useState<S3Provider>(
    profile?.protocol === "s3" ? guessProvider(profile.endpoint) : "aws"
  );
  // Azure-only state.
  const [azureAccount, setAzureAccount] = useState(profile?.account ?? "");

  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useDialog(panelRef, { onClose });

  const onProtocolChange = (p: Protocol) => {
    setProtocol(p);
    if (!portTouched) {
      setPort(PROTOCOL_DEFAULT_PORT[p]);
    }
    if (isObjectProtocol(p)) {
      // Object stores only do key auth — coerce to password.
      setAuthKind("password");
    }
    if ((p === "ftp" || p === "ftps") && authKind !== "password") {
      setAuthKind("password");
    }
  };

  const onS3ProviderChange = (p: S3Provider) => {
    setS3Provider(p);
    const preset = S3_PROVIDER_PRESETS[p];
    setRegion(preset.defaultRegion);
    if (p === "aws") {
      setEndpoint("");
    }
  };

  const isFtp = protocol === "ftp" || protocol === "ftps";
  const isS3 = protocol === "s3";
  const isAzure = protocol === "azure";
  const isObject = isObjectProtocol(protocol);
  const isAgent = isAgentProtocol(protocol);

  /// Build the profile from the current form state. Shared by Save and by the
  /// pairing flow (which must persist the profile before it can pair by id).
  const buildProfile = (): ConnectionProfile => {
    let auth: AuthMethod;
    if (authKind === "password") {
      auth = { kind: "password", password };
    } else if (authKind === "key") {
      auth = { kind: "key", path: keyPath, passphrase: passphrase || undefined };
    } else {
      auth = { kind: "agent" };
    }
    return {
      id,
      name:
        name ||
        (isS3
          ? `${bucket}@${s3Provider}`
          : isAzure
            ? `${azureAccount}/${bucket}`
            : isAgent
              ? `Agent @ ${host}`
              : `${username}@${host}`),
      protocol,
      host: isObject ? endpoint || (isAzure ? "blob.core.windows.net" : "s3.amazonaws.com") : host,
      port,
      username: isAzure ? azureAccount : isAgent ? "" : username,
      auth: isAgent ? { kind: "password", password: "" } : auth,
      defaultRemotePath: defaultRemotePath || undefined,
      color: profile?.color,
      autoConnect: autoConnect || undefined,
      bucket: isObject ? bucket : undefined,
      region: isS3 ? region : undefined,
      endpoint: isObject ? endpoint || undefined : undefined,
      account: isAzure ? azureAccount : undefined,
      agentKey: isAgent ? agentKey : undefined,
    };
  };

  const save = async () => {
    await saveProfile(buildProfile());
    onClose();
  };

  /// Pair with the daemon at host:port using a 6-digit code. Nothing is
  /// persisted until the daemon acknowledges, so a failed attempt leaves no
  /// half-configured connection behind. On success the profile is saved right
  /// away (pairing IS the consent step — demanding another Save was a dead
  /// end), the editor closes, and the connection opens.
  const pair = async (code: string): Promise<void> => {
    const res = await ipc.pairAgent(host.trim(), port, code);
    // buildProfile() reads state that React hasn't committed yet — pass the
    // fresh values through explicitly.
    const finalName = name || res.hostname || `Agent @ ${host}`;
    setAgentKey(res.serverKey);
    if (!name) setName(finalName);
    await saveProfile({ ...buildProfile(), name: finalName, agentKey: res.serverKey });
    toast.success(
      `Paired with ${res.hostname || host}`,
      `${res.os} · ${res.fingerprint}`
    );
    onClose();
    void connectProfile(id);
  };

  const canSave = isS3
    ? !!bucket && !!username && !!password
    : isAzure
      ? !!azureAccount && !!bucket && !!password
      : isAgent
        ? !!host && !!agentKey
        : !!host && !!username;
  // Name what's still required so a disabled Save isn't a dead end.
  const missing = (
    isS3
      ? [!bucket && "bucket", !username && "access key ID", !password && "secret key"]
      : isAzure
        ? [!azureAccount && "account", !bucket && "container", !password && "access key"]
        : isAgent
          ? [!host && "host", !agentKey && "pairing"]
          : [!host && "host", !username && "username"]
  ).filter(Boolean);

  return (
    <div className="fixed inset-0 z-modal flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="anim-modal max-h-[90vh] w-[32rem] max-w-[92vw] overflow-y-auto rounded-xl border border-border bg-bg-panel p-5 shadow-elev-3"
      >
        <div id={titleId} className="mb-4 text-sm font-semibold">
          {profile ? "Edit connection" : "New connection"}
        </div>

        <Field label="Name">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={isS3 ? "my-bucket" : "my-prod-box"}
            className={inputCls}
          />
        </Field>

        <Field label="Protocol">
          <div className="grid grid-cols-3 gap-1 rounded-md border border-border bg-bg-subtle p-1">
            {(["sftp", "ftp", "ftps", "s3", "azure", "faro-agent"] as Protocol[]).map((p) => (
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

        {isS3 ? (
          <S3Section
            provider={s3Provider}
            onProviderChange={onS3ProviderChange}
            bucket={bucket}
            setBucket={setBucket}
            region={region}
            setRegion={setRegion}
            endpoint={endpoint}
            setEndpoint={setEndpoint}
            accessKey={username}
            setAccessKey={setUsername}
            secretKey={password}
            setSecretKey={setPassword}
          />
        ) : isAzure ? (
          <AzureSection
            account={azureAccount}
            setAccount={setAzureAccount}
            container={bucket}
            setContainer={setBucket}
            accessKey={password}
            setAccessKey={setPassword}
            endpoint={endpoint}
            setEndpoint={setEndpoint}
          />
        ) : isAgent ? (
          <AgentSection
            profileId={id}
            host={host}
            setHost={setHost}
            port={port}
            setPort={setPort}
            setPortTouched={setPortTouched}
            paired={!!agentKey}
            onPair={pair}
            onUnpair={() => setAgentKey(undefined)}
          />
        ) : (
          <>
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
                    setPort(
                      parseInt(e.target.value) || PROTOCOL_DEFAULT_PORT[protocol]
                    );
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
                . Run <span className="font-mono">ssh-add</span> first to load
                your keys.
              </Hint>
            )}

            {authKind === "password" && (
              <Field label="Password">
                <PasswordInput value={password} onChange={setPassword} />
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
          </>
        )}

        <Field label={isObject ? "Default key prefix" : "Default remote path"}>
          <input
            value={defaultRemotePath}
            onChange={(e) => setDefaultRemotePath(e.target.value)}
            placeholder={isObject ? "" : isFtp ? "/" : "."}
            className={inputCls}
          />
        </Field>

        <label className="mb-3 flex cursor-pointer items-center gap-2.5 rounded-md border border-border bg-bg-subtle px-2.5 py-2">
          <input
            type="checkbox"
            checked={autoConnect}
            onChange={(e) => setAutoConnect(e.target.checked)}
            className="h-3.5 w-3.5 shrink-0 accent-[rgb(var(--accent))]"
          />
          <span className="min-w-0">
            <span className="block text-xs font-medium text-text">
              Auto-connect on startup
            </span>
            <span className="block text-[11px] text-text-dim">
              Open this server automatically when Faro launches.
            </span>
          </span>
        </label>

        {isFtp && (
          <Hint tone="warn">
            <ShieldOff size={11} className="mr-1 inline-block align-text-bottom" />
            FTP has no integrated terminal and{" "}
            {protocol === "ftp"
              ? "transmits credentials in clear text"
              : "uses TLS for the control channel only"}
            . Prefer SFTP when the server supports it.
          </Hint>
        )}

        <div className="mt-5 flex items-center justify-end gap-2">
          {!canSave && (
            <span className="mr-auto text-[11px] text-text-dim">
              Needs {missing.join(", ")}
            </span>
          )}
          <button
            className="rounded-md border border-border px-3.5 py-1.5 text-sm hover:bg-bg-hover"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            className="btn-accent rounded-md px-3.5 py-1.5 text-sm font-medium text-white disabled:opacity-40 disabled:cursor-not-allowed"
            onClick={save}
            disabled={!canSave}
            title={canSave ? undefined : `Fill in: ${missing.join(", ")}`}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}

/// Faro Agent connection editor: host/port, LAN discovery, and the pairing
/// ceremony. Controls a whole remote machine (no login/auth) — it's paired once
/// with a 6-digit code the daemon prints, then keyed by the pinned daemon key.
function AgentSection({
  profileId,
  host,
  setHost,
  port,
  setPort,
  setPortTouched,
  paired,
  onPair,
  onUnpair,
}: {
  profileId: string;
  host: string;
  setHost: (v: string) => void;
  port: number;
  setPort: (v: number) => void;
  setPortTouched: (v: boolean) => void;
  paired: boolean;
  onPair: (code: string) => Promise<void>;
  onUnpair: () => void;
}) {
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [found, setFound] = useState<DiscoveredAgent[] | null>(null);
  const codeRef = useRef<HTMLInputElement>(null);

  const scan = useCallback(async () => {
    setScanning(true);
    try {
      setFound(await ipc.discoverAgents());
    } catch (e) {
      toast.error("Scan failed", String(e));
    } finally {
      setScanning(false);
    }
  }, []);

  // Scan the moment the Faro Agent form opens — finding machines shouldn't
  // require knowing there's a button for it.
  useEffect(() => {
    void scan();
  }, [scan]);

  const doPair = async () => {
    setPairing(true);
    try {
      await onPair(code.trim());
      setCode("");
    } catch (e) {
      toast.error("Pairing failed", String(e));
    } finally {
      setPairing(false);
    }
  };

  const codeValid = /^\d{6}$/.test(code.trim());
  // The machine the host/port fields currently point at, if it was discovered.
  const selectedMachine = found?.find((d) => d.host === host && d.port === port);

  return (
    <>
      <div className="flex gap-2">
        <Field label="Host / IP" className="flex-1">
          <input
            value={host}
            onChange={(e) => setHost(e.target.value)}
            placeholder="192.168.1.42"
            className={inputCls}
          />
        </Field>
        <Field label="Port" className="w-24">
          <input
            type="number"
            value={port}
            onChange={(e) => {
              setPort(parseInt(e.target.value) || PROTOCOL_DEFAULT_PORT["faro-agent"]);
              setPortTouched(true);
            }}
            className={inputCls}
          />
        </Field>
      </div>

      {/* LAN discovery — runs automatically on open; the button rescans. */}
      <div className="mb-3">
        <button
          type="button"
          onClick={scan}
          disabled={scanning}
          className="flex items-center gap-1.5 rounded-md border border-border bg-bg-subtle px-2.5 py-1.5 text-xs text-text-muted hover:bg-bg-hover hover:text-text disabled:opacity-50"
        >
          {scanning ? (
            <Loader2 size={12} className="animate-spin" />
          ) : (
            <Radar size={12} />
          )}
          {scanning ? "Scanning…" : found ? "Rescan network" : "Scan local network"}
        </button>
        {found && found.length === 0 && !scanning && (
          <div className="mt-1.5 text-[11px] text-text-dim">
            No machines found. Start <span className="font-mono">faro-agentd</span> on
            the machine you want to control (or enable Remote control in its Faro
            app), or enter its IP above.
          </div>
        )}
        {found && found.length > 0 && (
          <div className="mt-1.5 flex flex-col gap-1">
            {found.map((d) => {
              const selected = d.host === host && d.port === port;
              const isThisProfile = d.pairedProfileId === profileId;
              return (
                <button
                  key={d.fingerprint + d.host}
                  type="button"
                  onClick={() => {
                    setHost(d.host);
                    setPort(d.port);
                    setPortTouched(true);
                    // Lead the user to the next step instead of silently
                    // filling a field they may not be looking at.
                    if (!paired) codeRef.current?.focus();
                  }}
                  className={cn(
                    "flex items-center justify-between rounded-md border px-2.5 py-1.5 text-left text-xs transition-colors",
                    selected
                      ? "border-accent bg-accent/10"
                      : "border-border bg-bg-subtle hover:bg-bg-hover"
                  )}
                >
                  <span className="min-w-0">
                    <span className="flex min-w-0 items-center gap-1.5">
                      <span className="truncate font-medium text-text">
                        {d.hostname || d.host}
                      </span>
                      {selected && <Check size={11} className="shrink-0 text-accent" />}
                    </span>
                    <span className="block truncate text-[10px] text-text-dim">
                      {d.os} · {d.host}:{d.port} · {d.fingerprint}
                    </span>
                  </span>
                  {d.pairedProfileId ? (
                    <span className="ml-2 shrink-0 rounded-full border border-success/40 bg-success/10 px-1.5 py-0.5 text-[9px] font-medium text-success">
                      {isThisProfile ? "this connection" : "paired"}
                    </span>
                  ) : d.pairable ? (
                    <span className="ml-2 shrink-0 rounded-full border border-accent/40 bg-accent/10 px-1.5 py-0.5 text-[9px] font-medium text-accent">
                      ready to pair
                    </span>
                  ) : (
                    <MonitorSmartphone size={13} className="ml-2 shrink-0 text-text-dim" />
                  )}
                </button>
              );
            })}
          </div>
        )}
      </div>

      {/* Pairing */}
      {paired ? (
        <div className="mb-3 flex items-center justify-between rounded-md border border-success/30 bg-success/10 px-2.5 py-2">
          <span className="flex items-center gap-1.5 text-xs text-text">
            <Check size={13} className="text-success" />
            Paired — the daemon's key is pinned.
          </span>
          <button
            type="button"
            onClick={onUnpair}
            className="text-[11px] text-text-dim underline hover:text-text"
          >
            Re-pair
          </button>
        </div>
      ) : (
        <div className="mb-3 rounded-md border border-border bg-bg-subtle p-2.5">
          <div className="mb-1.5 flex items-center gap-1.5 text-xs font-medium text-text">
            <Link2 size={13} className="text-accent" />
            {selectedMachine
              ? `Pair with ${selectedMachine.hostname || host}`
              : "Pair with this machine"}
          </div>
          <div className="mb-2 text-[11px] leading-relaxed text-text-dim">
            {selectedMachine && selectedMachine.pairable === false ? (
              <>
                <span className="font-medium text-text-muted">
                  {selectedMachine.hostname || host}
                </span>{" "}
                isn't accepting pairing right now. On that machine run{" "}
                <span className="font-mono text-text-muted">faro-agentd pair</span> (or
                open its Faro app → Settings → Remote control → Show pairing code),
                then type the code here.
              </>
            ) : selectedMachine ? (
              <>
                Type the 6-digit code showing on{" "}
                <span className="font-medium text-text-muted">
                  {selectedMachine.hostname || host}
                </span>
                . No code there? Run{" "}
                <span className="font-mono text-text-muted">faro-agentd pair</span> on
                it, or open its Faro app → Settings → Remote control → Show pairing
                code.
              </>
            ) : (
              <>
                On the machine you want to control, run{" "}
                <span className="font-mono text-text-muted">faro-agentd pair</span> (or
                open its Faro app → Settings → Remote control → Show pairing code) and
                type the 6-digit code below. Pairing sets up an end-to-end encrypted
                link and pins the machine's key — you won't need the code again.
              </>
            )}
          </div>
          <div className="flex gap-2">
            <input
              ref={codeRef}
              value={code}
              onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
              onKeyDown={(e) => {
                if (e.key === "Enter" && codeValid && host && !pairing) void doPair();
              }}
              placeholder="000000"
              inputMode="numeric"
              className={cn(inputCls, "flex-1 text-center font-mono tracking-[0.3em]")}
            />
            <button
              type="button"
              onClick={doPair}
              disabled={!codeValid || !host || pairing}
              className="btn-accent flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm font-medium text-white disabled:cursor-not-allowed disabled:opacity-40"
            >
              {pairing && <Loader2 size={13} className="animate-spin" />}
              {pairing ? "Pairing…" : "Pair"}
            </button>
          </div>
        </div>
      )}
    </>
  );
}

function AzureSection({
  account,
  setAccount,
  container,
  setContainer,
  accessKey,
  setAccessKey,
  endpoint,
  setEndpoint,
}: {
  account: string;
  setAccount: (v: string) => void;
  container: string;
  setContainer: (v: string) => void;
  accessKey: string;
  setAccessKey: (v: string) => void;
  endpoint: string;
  setEndpoint: (v: string) => void;
}) {
  return (
    <>
      <Hint>
        Azure Blob Storage. Use the storage account name and an account key
        (Portal → Storage account → Access keys). Custom endpoints target
        Azurite (local emulator) or sovereign clouds.
      </Hint>

      <div className="flex gap-2">
        <Field label="Storage account" className="flex-1">
          <input
            value={account}
            onChange={(e) => setAccount(e.target.value)}
            placeholder="mystorageacct"
            className={inputCls}
          />
        </Field>
        <Field label="Container" className="flex-1">
          <input
            value={container}
            onChange={(e) => setContainer(e.target.value)}
            placeholder="my-container"
            className={inputCls}
          />
        </Field>
      </div>

      <Field label="Access key">
        <input
          type="password"
          value={accessKey}
          onChange={(e) => setAccessKey(e.target.value)}
          className={inputCls}
        />
      </Field>

      <Field label="Endpoint (optional)">
        <input
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
          placeholder="(leave blank for public Azure)"
          className={inputCls}
        />
      </Field>
    </>
  );
}

function S3Section({
  provider,
  onProviderChange,
  bucket,
  setBucket,
  region,
  setRegion,
  endpoint,
  setEndpoint,
  accessKey,
  setAccessKey,
  secretKey,
  setSecretKey,
}: {
  provider: S3Provider;
  onProviderChange: (p: S3Provider) => void;
  bucket: string;
  setBucket: (v: string) => void;
  region: string;
  setRegion: (v: string) => void;
  endpoint: string;
  setEndpoint: (v: string) => void;
  accessKey: string;
  setAccessKey: (v: string) => void;
  secretKey: string;
  setSecretKey: (v: string) => void;
}) {
  const preset = S3_PROVIDER_PRESETS[provider];
  return (
    <>
      <Field label="Provider">
        <div className="grid grid-cols-3 gap-1 rounded-md border border-border bg-bg-subtle p-1">
          {(["aws", "r2", "b2"] as S3Provider[]).map((p) => {
            const data = S3_PROVIDER_PRESETS[p];
            return (
              <button
                key={p}
                type="button"
                onClick={() => onProviderChange(p)}
                className={
                  "flex flex-col items-start rounded-sm px-2 py-1.5 text-left transition-colors " +
                  (provider === p
                    ? "bg-accent-soft text-text ring-1 ring-inset ring-accent/40"
                    : "text-text-muted hover:bg-bg-hover hover:text-text")
                }
              >
                <span className="flex items-center gap-1 text-[11px] font-semibold">
                  <Cloud size={11} className={provider === p ? "text-accent" : ""} />
                  {data.label}
                </span>
                <span className="text-[10px] text-text-dim">{p === "aws" ? "Amazon" : p === "r2" ? "Cloudflare" : "Backblaze"}</span>
              </button>
            );
          })}
        </div>
      </Field>

      <Hint>{preset.description}</Hint>

      <Field label="Bucket">
        <input
          value={bucket}
          onChange={(e) => setBucket(e.target.value)}
          placeholder="my-bucket"
          className={inputCls}
        />
      </Field>

      <div className="flex gap-2">
        <Field label="Region" className="w-32">
          <input
            value={region}
            onChange={(e) => setRegion(e.target.value)}
            className={inputCls}
          />
        </Field>
        <Field label={provider === "aws" ? "Endpoint (optional)" : "Endpoint"} className="flex-1">
          <input
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder={preset.endpointHint}
            className={inputCls}
          />
        </Field>
      </div>

      <Field label="Access key ID">
        <input
          value={accessKey}
          onChange={(e) => setAccessKey(e.target.value)}
          placeholder="AKIA…"
          className={inputCls}
        />
      </Field>
      <Field label="Secret access key">
        <input
          type="password"
          value={secretKey}
          onChange={(e) => setSecretKey(e.target.value)}
          className={inputCls}
        />
      </Field>
    </>
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
    case "s3":
      return "Object · :443";
    case "azure":
      return "Blob · :443";
    case "faro-agent":
      return "Machine · :8722";
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
  let Icon = TerminalIcon;
  if (label === "FTPS") Icon = ShieldCheck;
  else if (label === "FTP") Icon = ShieldOff;
  else if (label === "S3" || label === "Azure") Icon = Cloud;
  else if (label === "Faro Agent") Icon = MonitorSmartphone;
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
        <Icon size={11} className={active ? "text-accent" : ""} />
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

/**
 * Password input with a reveal toggle and a "Generate strong password" action
 * that fills the field and copies the value to the clipboard — for the common
 * case of creating a brand-new server account while adding the connection.
 */
function PasswordInput({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [reveal, setReveal] = useState(false);
  const [copied, setCopied] = useState(false);

  async function generate() {
    const pw = generatePassword();
    onChange(pw);
    setReveal(true); // so the user can see/verify what was generated
    try {
      await navigator.clipboard.writeText(pw);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be unavailable (focus/permissions); the field is still set.
    }
  }

  return (
    <div>
      <div className="relative">
        <input
          type={reveal ? "text" : "password"}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className={cn(inputCls, "pr-9")}
          autoComplete="off"
          spellCheck={false}
        />
        <button
          type="button"
          onClick={() => setReveal((r) => !r)}
          title={reveal ? "Hide password" : "Show password"}
          aria-label={reveal ? "Hide password" : "Show password"}
          className="absolute inset-y-0 right-0 flex items-center px-2.5 text-text-dim hover:text-text"
        >
          {reveal ? <EyeOff size={13} /> : <Eye size={13} />}
        </button>
      </div>
      <button
        type="button"
        onClick={generate}
        className="mt-1.5 inline-flex items-center gap-1 rounded-md border border-border bg-bg-subtle px-2 py-1 text-[11.5px] text-text-muted transition-colors hover:bg-bg-hover hover:text-text"
      >
        {copied ? (
          <Check size={11} className="text-accent" />
        ) : (
          <Wand2 size={11} />
        )}
        {copied ? "Generated & copied" : "Generate strong password"}
      </button>
    </div>
  );
}
