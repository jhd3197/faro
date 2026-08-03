# HubSpot private app: required scopes

Faro connects to a HubSpot portal with a **private-app access token**
(`pat-…`). The token alone is not enough — the private app must also be
granted the scopes for the surfaces you want to browse. Each scope unlocks
one root in Faro; any single one is enough to connect, and roots without
their scope simply stay hidden.

## Setup

1. In the HubSpot portal: **Settings → Integrations → Private Apps**.
2. Open the app (or create one) → **Scopes** tab.
3. Enable the scopes below, then **Save**. Scope changes take effect
   immediately — no reinstall, and the token does not change.

## Scopes

| Scope | Unlocks in Faro | Notes |
| --- | --- | --- |
| `content` | `design (draft)` + `design (published)` — Design Manager source files (templates, modules, CSS/JS, HubL) | Read/write. Writes to `design (published)` deploy instantly, like pressing Publish in the Design Manager. |
| `files` | `files` — the File Manager asset library | Read/write (upload, rename, delete). |
| `hubdb` | `hubdb` — HubDB tables as read-only `.csv` files | Read-only in Faro for now. |

`content`, `files`, and `hubdb` appear in the scope picker under their CMS /
files / HubDB groupings; searching the scope name in the picker's filter box
is the fastest way to find them.

## Troubleshooting

- **"HubSpot rejected the token (401)"** — the pasted value isn't a valid
  private-app token. Private-app tokens start with `pat-`; old HubSpot API
  keys (`hapikey`) were retired in November 2022 and no longer work.
- **A root is missing after connect, or entering it says it "needs the
  `…` scope"** — the app lacks that scope; enable it and reconnect.
- **"The token is valid, but the private app has none of the scopes Faro
  uses"** — none of the three scopes above are enabled.
