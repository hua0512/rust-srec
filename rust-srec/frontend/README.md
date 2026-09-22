# Rust-Srec frontend

React frontend for Rust-Srec, built with TanStack Router, TanStack Start,
Tailwind CSS, and Zustand. The web build uses SSR; the Tauri desktop build
uses a separate client-only entry point.

Use the Node version in [`.nvmrc`](../../.nvmrc) and the pnpm version in
[`package.json`](package.json). Run these commands from this directory:

```sh
pnpm install --frozen-lockfile
pnpm dev
```

The development server listens on port 15275 and proxies `/api` to
`http://127.0.0.1:12555`. See [`.env.example`](.env.example) for configuration.

```sh
pnpm test                  # Vitest; append a test path to focus the run
pnpm lint                  # oxlint, including type-aware rules
pnpm fmt:check             # oxfmt
pnpm typecheck
pnpm build                 # production web build
pnpm build:desktop         # production desktop frontend
```

See [AGENTS.md](AGENTS.md) for source layout, generated-file boundaries,
and validation guidance, and [CONTRIBUTING.md](../../CONTRIBUTING.md) for
repository setup.
