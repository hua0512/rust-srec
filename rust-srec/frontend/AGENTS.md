# Frontend Agent Guide

## OVERVIEW

- Vite-powered React app using **TanStack Start** (SSR/Streaming) and **TanStack Router**.
- Hybrid deployment: SSR (web) and CSR (Tauri desktop).
- Styling: Tailwind CSS v4 + shadcn/ui.
- Linting/Formatting: **oxlint** and **oxfmt** (fast Rust-based tooling).

## STRUCTURE

- `src/routes/`: File-based routing (TanStack Router).
- `src/server/functions/`: Server-side logic (TanStack Start server functions).
- `src/api/proto/`: Generated Protobuf files for real-time progress/logs.
- `src/components/ui/`: shadcn/ui base components.
- `src/store/`: Global state management (Zustand).
- `src/hooks/`: Reusable React hooks.

## WHERE TO LOOK

- `src/routeTree.gen.ts`: AUTO-GENERATED route definitions. Do not edit.
- `src/api/proto/*.js`: AUTO-GENERATED Protobuf modules. Regenerate via `proto:gen`.
- `router.tsx` / `router.desktop.tsx`: Router configuration for web vs desktop.
- `main.desktop.tsx`: Entry point for Tauri desktop build.

## CONVENTIONS

- **File-based Routing**: Place new routes in `src/routes/`.
- **Server Functions**: Use `src/server/functions/` for API calls/backend logic.
- **Type Safety**: Prefer Zod schemas for validation and TanStack Query for data fetching.
- **Components**: Follow shadcn/ui patterns; use `cn()` utility for Tailwind class merging.
- **Performance**: Minimize client-side state; leverage SSR and streaming where possible.

## ANTI-PATTERNS

- **Manual Route Trees**: Never edit `routeTree.gen.ts` manually.
- **Direct Proto Edits**: Never edit `src/api/proto/*.js` directly. Use `pnpm proto:gen`.
- **Standard Lint**: Avoid ESLint; stick to **oxlint** for speed and consistency.

## COMMANDS

- `pnpm dev`: Start dev server on http://localhost:15275.
- `pnpm build`: Production web build.
- `pnpm build:desktop`: Production desktop build (requires Tauri context).
- `pnpm proto:gen`: Regenerate Protobuf JavaScript modules.
- `pnpm lint`: Run oxlint.
- `pnpm format`: Run oxfmt.
- `pnpm test`: Run vitest.
