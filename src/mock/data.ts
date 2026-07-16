// Fictional dataset for the mock/demo build (VITE_MOCK=1). Everything here is
// made up — hostnames, IPs, paths, sizes — so screenshots never leak real data.
import type {
  BridgeStatus,
  ConnectionProfile,
  SyncPlan,
  Transfer,
} from "@/lib/types";
import type { Capabilities, DirEntry } from "@faro/file-ui";

const T = 1_719_700_000; // fixed base timestamp (sec) so listings are stable

export let profiles: ConnectionProfile[] = [
  {
    id: "p-api",
    name: "api-prod",
    protocol: "sftp",
    host: "10.0.0.12",
    port: 22,
    username: "root",
    auth: { kind: "agent" },
    defaultRemotePath: "/var/www/api",
    color: "#8b7ff6",
    autoConnect: true,
  },
  {
    id: "p-db",
    name: "db-staging",
    protocol: "sftp",
    host: "10.0.0.40",
    port: 22,
    username: "postgres",
    auth: { kind: "agent" },
    defaultRemotePath: "/var/lib/pgsql/data",
    color: "#4ade80",
  },
  {
    id: "p-edge",
    name: "web-edge",
    protocol: "sftp",
    host: "198.51.100.7",
    port: 22,
    username: "deploy",
    auth: { kind: "agent" },
    defaultRemotePath: "/srv/app",
    color: "#38bdf8",
  },
  {
    id: "p-ftp",
    name: "legacy-ftp",
    protocol: "ftp",
    host: "203.0.113.9",
    port: 21,
    username: "ftpuser",
    auth: { kind: "password", password: "" },
    defaultRemotePath: "/public_html",
    color: "#fbbf24",
  },
  {
    id: "p-s3",
    name: "acme-backups",
    protocol: "s3",
    host: "s3.amazonaws.com",
    port: 443,
    username: "",
    auth: { kind: "agent" },
    bucket: "acme-backups",
    region: "us-east-1",
    color: "#f472b6",
  },
  {
    id: "p-cdn",
    name: "media-cdn",
    protocol: "s3",
    host: "s3.amazonaws.com",
    port: 443,
    username: "",
    auth: { kind: "agent" },
    bucket: "media-cdn",
    region: "us-west-2",
    color: "#a78bfa",
  },
];

export function setProfiles(next: ConnectionProfile[]) {
  profiles = next;
}

// sessionId → profileId, populated by mock `connect`.
const sessions = new Map<string, string>();
export function openSession(profileId: string): string {
  const sid = `sess-${profileId}`;
  sessions.set(sid, profileId);
  return sid;
}
export function closeSession(sid: string) {
  sessions.delete(sid);
}
function protocolOf(sessionId: string): string {
  const pid = sessions.get(sessionId);
  return profiles.find((p) => p.id === pid)?.protocol ?? "sftp";
}

function f(name: string, size: number, mod: number, mode = 0o644): Omit<DirEntry, "path"> {
  return { name, kind: "file", size, modified: T - mod, mode };
}
function d(name: string, mod: number, mode = 0o755): Omit<DirEntry, "path"> {
  return { name, kind: "directory", size: 0, modified: T - mod, mode };
}

function join(path: string, name: string): string {
  if (/^[a-zA-Z]:/.test(path) || path.includes("\\")) {
    return path.endsWith("\\") ? `${path}${name}` : `${path}\\${name}`;
  }
  return path === "/" ? `/${name}` : `${path}/${name}`;
}

// Curated listings for the paths the screenshots land on; everything else gets a
// believable generic listing so navigation never dead-ends.
const REMOTE: Record<string, Array<Omit<DirEntry, "path">>> = {
  "/var/www/api": [
    d("node_modules", 86_400),
    d("src", 3_600),
    d("dist", 1_800),
    d("logs", 600),
    f("package.json", 1_842, 7_200),
    f("package-lock.json", 284_117, 7_200),
    f("server.js", 9_233, 3_600, 0o644),
    f("ecosystem.config.js", 1_204, 90_000),
    f(".env", 412, 5_400, 0o600),
    f("Dockerfile", 893, 172_800),
    f("README.md", 4_517, 172_800),
  ],
  "/srv/app": [
    d("releases", 7_200),
    d("shared", 7_200),
    d("current", 1_200),
    f("deploy.sh", 2_140, 3_600, 0o755),
    f("nginx.conf", 3_980, 86_400, 0o644),
  ],
};

const S3_ROOT: Array<Omit<DirEntry, "path">> = [
  d("db-backups", 3_600),
  d("media", 7_200),
  d("logs", 1_800),
  f("2026-06-29-full.tar.gz", 1_980_321_004, 3_600),
  f("latest.sql.gz", 412_889_233, 1_800),
  f("manifest.json", 2_044, 600),
];

const LOCAL_C: Array<Omit<DirEntry, "path">> = [
  d("Users", 8_640_000),
  d("Program Files", 8_640_000),
  d("Program Files (x86)", 8_640_000),
  d("Windows", 8_640_000),
  d("ProgramData", 8_640_000),
];
const LOCAL_GENERIC: Array<Omit<DirEntry, "path">> = [
  d("Documents", 86_400),
  d("Downloads", 3_600),
  d("Pictures", 172_800),
  d("Projects", 600),
  f("notes.txt", 1_204, 7_200),
  f("budget.xlsx", 38_402, 90_000),
];
const REMOTE_GENERIC: Array<Omit<DirEntry, "path">> = [
  d("bin", 864_000),
  d("etc", 864_000),
  d("var", 864_000),
  d("home", 864_000),
  f("config.yaml", 1_882, 7_200),
  f("changelog.md", 9_233, 86_400),
];

export function listDir(sessionId: string, path: string): DirEntry[] {
  const proto = protocolOf(sessionId);
  let base: Array<Omit<DirEntry, "path">>;
  if (sessionId === "local") {
    base = path === "C:\\" || path === "/" ? LOCAL_C : LOCAL_GENERIC;
  } else if (proto === "s3" || proto === "azure" || proto === "gcs") {
    base = S3_ROOT;
  } else {
    base = REMOTE[path] ?? REMOTE_GENERIC;
  }
  return base.map((e) => ({ ...e, path: join(path, e.name) }));
}

export function capabilities(sessionId: string): Capabilities {
  const proto = protocolOf(sessionId);
  if (sessionId === "local") {
    return { canChmod: false, canSymlink: true, canRename: true, hasDirectories: true, hasShell: false };
  }
  if (proto === "s3" || proto === "azure" || proto === "gcs") {
    return { canChmod: false, canSymlink: false, canRename: true, hasDirectories: false, hasShell: false };
  }
  if (proto === "ftp" || proto === "ftps") {
    return { canChmod: true, canSymlink: false, canRename: true, hasDirectories: true, hasShell: false };
  }
  return { canChmod: true, canSymlink: true, canRename: true, hasDirectories: true, hasShell: true };
}

export const transfers: Transfer[] = [
  {
    id: "t1",
    kind: "download",
    source: "/var/www/api/dist.tar.gz",
    destination: "C:\\Users\\demo\\Downloads\\dist.tar.gz",
    size: 52_428_800,
    transferred: 34_603_008,
    status: "transferring",
    startedAt: (T - 20) * 1000,
  },
  {
    id: "t2",
    kind: "download",
    source: "/var/www/api/logs/api.log",
    destination: "C:\\Users\\demo\\Downloads\\api.log",
    size: 1_204_993,
    transferred: 1_204_993,
    status: "done",
    startedAt: (T - 90) * 1000,
  },
  {
    id: "t3",
    kind: "upload",
    source: "C:\\Users\\demo\\Projects\\config.yaml",
    destination: "/srv/app/shared/config.yaml",
    size: 1_882,
    transferred: 1_882,
    status: "done",
    startedAt: (T - 140) * 1000,
  },
  {
    id: "t4",
    kind: "download",
    source: "/var/www/api/node_modules.zip",
    destination: "C:\\Users\\demo\\Downloads\\node_modules.zip",
    size: 184_549_376,
    transferred: 0,
    status: "queued",
    startedAt: (T - 5) * 1000,
  },
];

export const bridgeStatus: BridgeStatus = {
  running: true,
  enabled: true,
  url: "http://127.0.0.1:8765",
  port: 8765,
  token: "demo-7f3a1c9b",
  enabledSessions: ["sess-p-api"],
  policy: { allowAll: false, autoRead: true, autoSafeExec: false, allowSudo: false },
};

export const bridgeActivity = [
  { id: "a1", sessionId: "sess-p-api", kind: "exec", detail: "systemctl status api", ok: true, at: (T - 30) * 1000 },
  { id: "a2", sessionId: "sess-p-api", kind: "read", detail: "/var/www/api/.env", ok: true, at: (T - 70) * 1000 },
  { id: "a3", sessionId: "sess-p-api", kind: "denied", detail: "rm -rf /var/www/api/dist", ok: false, at: (T - 110) * 1000 },
];

export const savedCommands = [
  { id: "c1", name: "deploy", command: "cd /srv/app && ./deploy.sh", description: "Pull + restart the app" },
  { id: "c2", name: "tail-errors", command: "tail -n 100 /var/www/api/logs/error.log", description: "Last 100 error lines" },
];

export function syncPlan(localRoot: string, remoteRoot: string): SyncPlan {
  return {
    direction: "localToRemote",
    strategy: "additive",
    localRoot,
    remoteRoot,
    copies: [
      { relative: "dist/app.js", sourcePath: `${localRoot}\\dist\\app.js`, destinationPath: `${remoteRoot}/dist/app.js`, size: 482_004, reason: "newer" },
      { relative: "dist/app.css", sourcePath: `${localRoot}\\dist\\app.css`, destinationPath: `${remoteRoot}/dist/app.css`, size: 91_233, reason: "sizeChanged" },
      { relative: "public/logo.svg", sourcePath: `${localRoot}\\public\\logo.svg`, destinationPath: `${remoteRoot}/public/logo.svg`, size: 4_117, reason: "missing" },
    ],
    deletes: [
      { relative: "dist/old-bundle.js", path: `${remoteRoot}/dist/old-bundle.js`, kind: "file", size: 220_440 },
    ],
    totalBytes: 577_354,
  };
}

// A short colored shell transcript emitted when a mock terminal opens.
export const TERMINAL_TRANSCRIPT =
  "\x1b[1;32mroot@api-prod\x1b[0m:\x1b[1;34m/var/www/api\x1b[0m# ls -la\r\n" +
  "total 312\r\n" +
  "drwxr-xr-x  8 root root   4096 Jun 29 14:02 \x1b[1;34m.\x1b[0m\r\n" +
  "drwxr-xr-x  3 root root   4096 Jun 12 09:18 \x1b[1;34m..\x1b[0m\r\n" +
  "-rw-------  1 root root    412 Jun 29 12:40 .env\r\n" +
  "drwxr-xr-x  4 root root   4096 Jun 29 13:55 \x1b[1;34mdist\x1b[0m\r\n" +
  "drwxr-xr-x 312 root root  12288 Jun 28 22:10 \x1b[1;34mnode_modules\x1b[0m\r\n" +
  "-rw-r--r--  1 root root   1842 Jun 29 11:20 package.json\r\n" +
  "-rw-r--r--  1 root root   9233 Jun 29 13:54 server.js\r\n" +
  "\x1b[1;32mroot@api-prod\x1b[0m:\x1b[1;34m/var/www/api\x1b[0m# uptime\r\n" +
  " 14:02:51 up 37 days,  2:14,  1 user,  load average: 0.08, 0.12, 0.09\r\n" +
  "\x1b[1;32mroot@api-prod\x1b[0m:\x1b[1;34m/var/www/api\x1b[0m# \x1b[5m▋\x1b[0m";
