# Third-party licenses

Faro bundles assets and code from third parties. Their licenses are reproduced
below. (npm dependencies retain their own licenses in `node_modules`; this file
covers redistributed **assets** that ship inside the built app.)

## Material Icon Theme

File-type icons in the file browser are from
[Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme)
(`material-icon-theme` on npm), used under the MIT License.

```
The MIT License (MIT)
Copyright (c) 2025 Material Extensions

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## Iconify brand & protocol logos (Plan 14)

Recognizable brand/protocol marks on connections (AWS S3, Azure, Google Cloud,
Dropbox, OneDrive, Google Drive, Box, an SSH glyph, the Faro lighthouse), plus
cloud-storage vendor marks on the New-Connection provider presets (Cloudflare,
Backblaze, DigitalOcean, Wasabi, MinIO, Hetzner, Scaleway, Oracle, IBM, Supabase,
Nextcloud, ownCloud), come from
[Iconify](https://iconify.design). Faro bundles only the handful of icons it
references (extracted offline by `scripts/gen-brand-icons.mjs` into
`src/lib/brandIconData.ts`) — it makes **no** network calls to the Iconify API.
Only permissively-licensed sets are used:

- **logos** (colour brand marks) — CC0-1.0 (public domain dedication).
  <https://github.com/gilbarbara/logos>
- **Simple Icons** (monochrome brand marks) — CC0-1.0 (public domain dedication).
  <https://github.com/simple-icons/simple-icons>
- **Material Design Icons (mdi)** (neutral protocol glyphs) — used under the
  Apache License 2.0. <https://github.com/Templarian/MaterialDesign>

```
                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   Licensed under the Apache License, Version 2.0 (the "License"); you may not
   use these files except in compliance with the License. You may obtain a copy
   of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
   WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
   License for the specific language governing permissions and limitations under
   the License.
```

Brand marks are trademarks of their respective owners; they are used here only
to identify the corresponding service in the connection UI.
