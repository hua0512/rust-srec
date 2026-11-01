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
