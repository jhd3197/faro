# Updater signing key — custody & setup

Faro's in-app auto-updater (Plan 16) will only install an update whose artifact
carries a valid **minisign signature** matching the public key baked into
`src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). Signature verification is
the entire security model: the private key that signs releases must stay secret,
and it must never be lost.

## Current state

- A signing keypair has been generated. The **public** key is committed in
  `src-tauri/tauri.conf.json`.
- The **private** key lives at `.updater/faro-updater.key` (gitignored — it is
  **not** in the repo) and was generated with an **empty password**.
- CI (`.github/workflows/release.yml`) produces signed updater artifacts +
  `latest.json` **only when the signing secret is present** (see below). Until
  then, releases build exactly as before, just without the in-app update path —
  so nothing is broken while the key is being wired up.

## To turn on in-app updates (one-time, maintainer only)

Add two repository secrets under **Settings → Secrets and variables → Actions**:

| Secret | Value |
|--------|-------|
| `TAURI_SIGNING_PRIVATE_KEY` | the full contents of `.updater/faro-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | empty string (the key has no password) |

The next push to `main` then builds signed artifacts and uploads
`latest.json` to the GitHub Release. The updater endpoint is already configured:

```
https://github.com/jhd3197/Faro/releases/latest/download/latest.json
```

## Recommended: rotate before the first public release

This keypair was generated inside a development session. If there is **any**
concern that the private key could have been exposed, regenerate a fresh one and
replace the public key before shipping the first update-capable release:

```bash
npx tauri signer generate -w .updater/faro-updater.key -p "" --ci -f
cat .updater/faro-updater.key.pub          # paste this into tauri.conf.json pubkey
```

Then update the `TAURI_SIGNING_PRIVATE_KEY` secret to the new key's contents.

## Custody rules

- **Never commit** the private key or paste it anywhere public. `.updater/` is
  gitignored; keep it that way.
- **Back it up** somewhere safe (a password manager). If it is lost, existing
  installs can never be updated again — you'd have to ship a new key in a new
  release that users install manually.
- The public endpoint is served by GitHub Releases (HTTPS, maintainer-controlled),
  so a leaked private key alone is not enough to push a malicious update — but
  treat it as sensitive regardless.
