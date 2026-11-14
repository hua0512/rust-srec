# Showcase screenshots

`capture.mjs` produces the macOS-style screenshots used in the READMEs and on
the docs home page. Each page is captured from the real web UI in every
interface language and in a dark and a light variant, at 2× scale, and placed
in a window frame (`frame.html`). Images are written to
`rust-srec/docs/public/images/showcase/<locale>/<page>-<theme>.webp`; the
English README and docs use `en/`, the Chinese ones `zh-CN/`.

The data is fictional. Streamers, titles, chat statistics and avatars come
from `seed.mjs`, and the streamer URLs point at channels that do not exist.
The backend runs these pipeline jobs for real on generated test clips. Nothing
contacts a streaming platform or a remote deployment.

## Requirements

- A debug build of the backend: `cargo build --locked -p rust-srec --bin rust-srec`
- Frontend dependencies: `pnpm install --frozen-lockfile` in `rust-srec/frontend`
- Chromium (or Chrome) and `ffmpeg` on `PATH`
- Node from the repository's `.nvmrc`. The scripts use only Node built-ins.

## Usage

From the repository root:

```sh
node scripts/showcase/capture.mjs                       # all shots, every language and theme
node scripts/showcase/capture.mjs --only dashboard,jobs --theme dark --locale zh-CN
node scripts/showcase/capture.mjs --out /tmp/shots --raw  # also keep unframed PNGs
node scripts/showcase/capture.mjs --keep                # leave the servers running to inspect
```

Options: `--backend <path>` (default `target/debug/rust-srec`), `--chromium <binary>`
(default `chromium`), `--out <dir>`, `--theme dark,light`, `--locale en,zh-CN`, `--only <names>`,
`--raw` and `--keep`.

A run takes a few minutes, most of it the Vite dev server's first compile and
the pipeline jobs. Re-run it after UI changes and commit the updated images.

## How it works

1. The backend starts with `AUTH_DISABLED=true` on a temporary SQLite database.
   The first start applies migrations, `seed.mjs` writes the demo data, and a
   second start loads it.
2. `seed.mjs` then submits pipeline workflows through the REST API. Remux and
   thumbnail jobs complete. Uploads fail because no rclone remote exists. One
   slow encode keeps running, with another queued behind it.
3. `proxy.mjs` sits between the frontend and the backend:
   - It reports the seeded live streamers as live, because the backend resets
     them at startup when no recording is running.
   - It answers the download-progress WebSocket with fictional in-progress
     recordings.
   - It replaces the temporary directory with `/data` in JSON responses.
   Streamers on Chinese platforms get Chinese chat words and gift names, and
   the chat statistics shot shows such a stream in the Chinese captures.
4. The frontend runs under `vite dev`, with both backend URLs pointed at the
   proxy. This overrides `.env`, which may target a real deployment.
5. `session.mjs` mints the session cookie, and `cdp.mjs` drives headless
   Chromium. The language is chosen with the frontend's `app-locale` cookie.
   Each page in `shots.mjs` is captured, then screenshotted again inside
   `frame.html`, and written out as WebP.

To add a page, append an entry to `shots.mjs`. `ready` is a JavaScript
expression polled until it is true. `prepare` runs before the capture, for
example to scroll or switch a tab. Give the window `title` for every locale,
and match on attributes rather than visible text where possible; text that must
be matched goes in `TEXT`, per locale. To add an interface language, add it to
`LOCALES` and give every shot a title in it.
