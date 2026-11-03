# Frontend Agent Guide

Applies to `rust-srec/frontend/`. Use the root rules for shared workflow and completion; backend-specific service rules in the parent directory do not govern frontend work.

## OVERVIEW

- Vite-powered React app using **TanStack Start** (SSR/Streaming) and **TanStack Router**.
- Hybrid deployment: SSR (web) and CSR (Tauri desktop).
- Styling: Tailwind CSS v4 + shadcn/ui.
- Linting/Formatting: **oxlint** and **oxfmt** (fast Rust-based tooling).

## STRUCTURE

- `src/routes/`: File-based routing (TanStack Router).
- `src/server/functions/`: Server-side logic (TanStack Start server functions).
- `src/api/proto/gen/`: Generated Protobuf TypeScript files for real-time progress/logs.
- `src/components/ui/`: shadcn/ui base components.
- `src/store/`: Global state management (Zustand).
- `src/hooks/`: Reusable React hooks.

## WHERE TO LOOK

- `src/routeTree.gen.ts`: AUTO-GENERATED route definitions.
- `src/api/proto/gen/*_pb.ts`: AUTO-GENERATED Protobuf modules from `../proto`; `buf.gen.yaml` defines the output.
- `src/locales/{en,zh-CN}/messages.po`: Lingui catalogs; the runtime loads the compiled `messages.ts` beside each.
- `src/router.tsx` / `src/router.desktop.tsx`: Router configuration for web vs desktop.
- `src/main.desktop.tsx`: Entry point for Tauri desktop build.

## CONVENTIONS

- **File-based Routing**: Place new routes in `src/routes/`.
- **Server Functions**: Use `src/server/functions/` for API calls/backend logic.
- **Type Safety**: Prefer Zod schemas for validation and TanStack Query for data fetching.
- **Components**: Follow shadcn/ui patterns; use `cn()` utility for Tailwind class merging.
- **Performance**: Use SSR and streaming for the web deployment where useful; preserve the CSR path for the desktop deployment.

## ANTI-PATTERNS

- **Manual Route Trees**: Never edit `routeTree.gen.ts` manually.
- **Direct Proto Edits**: Never edit `src/api/proto/gen/*_pb.ts` directly. Use `pnpm proto:gen` after changing the source protos.
- **Standard Lint**: Avoid ESLint; stick to **oxlint** for speed and consistency.

## PITFALLS

Each of these has broken the app before and passes lint, types and unit tests.

- **Translations**: the runtime loads the compiled `src/locales/<locale>/messages.ts`, not the `.po`. After changing a `<Trans>`/`msg` string, run `pnpm extract`, fill the `zh-CN` msgstr (an empty one falls back to English), then `pnpm compile`, and commit `.po` and `.ts` for both locales. Generic words collide by msgid: give them a `context` (`<Trans context="pagination">Next</Trans>`). Hardcoded English often hides in `components/ui/*`.
- **Literal braces in `<Trans>`**: a string literal such as `{'{platform}'}` is inlined into the ICU message and renders as empty in every locale. Put the text in a module-level const and reference the identifier.
- **List-page state**: filters, search and pagination live in the URL. Use a non-lazy route file with `validateSearch`, `useUpdateSearch` (`@/hooks/use-update-search`) for writes, and `SearchInput` (`@/components/shared/search-input`), which debounces and is IME-safe for pinyin. Keep ephemeral UI (selection, dialogs) in `useState`.
- **Search-param validation**: use `searchParamsValidator` (`src/lib/search-params.ts`) with every field `.optional()`. `schema.parse` in `validateSearch` turns one bad value (`?page=abc`, a removed enum value in an old bookmark) into a full-page error with a Zod dump.
- **react-hook-form nested fields**: a field under a `null` parent object gets `null` (which `z.number().optional()` rejects, blocking submit), and registering a key the default object lacks makes `isDirty` true with empty `dirtyFields`. Seed every schema key (see `components/config/shared/danmu-statistics-value.ts`) and collapse all-undefined override objects back to `null` on submit. The global config PATCH treats `null` as omitted, so reset the form from the PATCH response.
- **Linked dependencies**: move `@tanstack/react-start`, `@tanstack/react-router` and `@tanstack/router-plugin` together.
- **Session cookie**: `src/utils/session.server.ts` owns the `srec_session` cookie (Iron seal via `iron-webcrypto`). It keeps one state per request, and a version check stops a slower seal from overwriting a newer write. The `{ data }` payload must keep unsealing cookies written by the older h3 session format, which a test fixture pins.
- **`MediaOutput.format`** is the file type (`VIDEO`, `AUDIO`, `THUMBNAIL`, `DANMU_XML`), never a container like `mp4`. Present it with `getMediaFileTypeMeta` (`src/lib/media-file-type.ts`); get a real extension from the path with `getMediaExtension` (`src/lib/media.ts`).
- **Card grids**: use the `CARD_GRID`, `COMPACT_CARD_GRID` and `STAT_GRID` constants from `src/lib/grid.ts`, never viewport `md:/lg:/xl:grid-cols-N`. Breakpoints ignore the sidebar and settings nav, so mid-size windows get cramped cards. Inside cards, `truncate` needs `min-w-0` or `grid-cols-1`.
- **`line-clamp-N`** sizes the content box while `overflow: hidden` clips at the padding box. Don't combine it with padding or a fixed height on the same element, or line N+1 paints into the padding.
- **Tailwind `group-hover:`** matches any `.group` ancestor. The desktop `Sidebar` wrapper in `components/ui/sidebar.tsx` is a plain `group`, so per-row hover effects inside it use named groups (`group/row`, `group-hover/row:`).
- **Style recalculation**: inherited changes on `body`/`main` restyle the whole page (Radix modal `pointer-events`, react-remove-scroll's body variable, `inert`), and flipping `data-state` re-matches every `data-state` selector in the subtree. Check a modal primitive or scroll lock for these before using it on a page-level overlay; that is why the mobile nav is `SidebarMobile`, not a Radix `Sheet`. Key an element's own state on a dedicated attribute (`data-drawer`).
- **Pre-paint scripts on the web build**: behind Cloudflare Rocket Loader every `<script>` is deferred past first paint unless the SSR markup carries `data-cfasync="false"` (setting it from JS is too late). The theme script spreads `THEME_SCRIPT_ATTRS` (`src/lib/theme-script.ts`); anything else that must run before paint needs the same. For a web-only visual bug, inspect the served HTML first.

## COMMANDS

Run from `rust-srec/frontend/`. Use Node from the root `.nvmrc` and pnpm from this directory's `package.json`.

- `pnpm dev`: Start dev server on http://localhost:15275.
- `pnpm build`: Production web build.
- `pnpm dev:desktop`: Desktop (CSR) dev server on http://127.0.0.1:15275, used by `cargo tauri dev`.
- `pnpm build:desktop`: Production desktop build into `dist-desktop/` (runs `proto:gen` first).
- `pnpm proto:gen`: Regenerate Protobuf TypeScript modules using Buf.
- `pnpm lint`: Run oxlint.
- `pnpm fmt`: Apply oxfmt; review the diff for unrelated formatting changes.
- `pnpm fmt:check`: Check formatting without rewriting files.
- `pnpm test`: Run vitest.
- `pnpm typecheck`: Check TypeScript types.
- `pnpm extract` / `pnpm compile`: After changing UI strings, extract the Lingui catalogs, translate `zh-CN`, then compile `messages.ts`.

## VALIDATION

For frontend behavior changes, run formatting checks, lint, and relevant tests (`pnpm test <test-file>` for a focused test). Run `pnpm typecheck` when types or interfaces change, `pnpm build` for routing, SSR, or bundling changes, and `pnpm build:desktop` for affected desktop integration. Shared frontend infrastructure or dependency changes warrant the wider test/build scope for the deployments they affect.

For prose-only edits, follow the root validation table. Regenerate protobuf outputs only when their sources or generation configuration change. CI's full frontend checks are maintained in `.github/workflows/pr.yml` at the repository root; the command list above is not a mandatory sequence for every edit.
