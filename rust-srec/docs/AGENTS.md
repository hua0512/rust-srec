# Documentation AGENTS.md

## OVERVIEW
VitePress-based documentation site for `rust-srec`. Multi-language (EN/ZH). 

## STRUCTURE
- `en/`, `zh/`: Markdown source for English and Chinese documentation.
- `.vitepress/`: VitePress engine, configuration, and theme.
  - `config.mts`: Central source of truth for sidebar, navbar, and locale routing.
- `public/`: Static assets (logos, diagrams, sample configuration files).

## WHERE TO LOOK
- `.vitepress/config.mts`: Update this for any sidebar or navigation changes.
- `en/getting-started/`: Core installation, Docker, and setup guides.
- `en/concepts/`: High-level architecture, pipeline, and notification logic.
- `public/`: Place images and shared assets here.

## CONVENTIONS / ANTI-PATTERNS
- **Sync**: Maintain parity between `en/` and `zh/` content and file structure.
- **Node Version**: CI uses **Node 20**. Ensure local environment compatibility.
- **Sidebars**: New pages must be manually added to `sidebar` in `config.mts`.
- **Assets**: Reference assets in `public/` using root-relative paths (e.g., `/stream-rec.svg`).
- **Dead Links**: Backend-managed links (e.g., `/api/docs`) are ignored via `ignoreDeadLinks` in config.

## COMMANDS
Run from `rust-srec/docs/`:
- `pnpm install --frozen-lockfile`: Install dependencies (CI parity).
- `pnpm run docs:dev`: Start local development server with hot reload.
- `pnpm run docs:build`: Build production-ready static site to `.vitepress/dist/`.
- `pnpm run docs:preview`: Preview the production build locally.

## NOTES
- **Generated Folders**: `.vitepress/dist/` and `.vitepress/cache/` are build/cache outputs and are git-ignored. 
- **Mermaid**: Support for diagrams is included via the `mermaid` dependency.
