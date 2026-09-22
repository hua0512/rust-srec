# Documentation AGENTS.md

Applies to `rust-srec/docs/`; backend-specific service rules in the parent directory do not govern docs work. Use the repository-wide workflow and read referenced material only for the task at hand.

## OVERVIEW
VitePress-based documentation site for `rust-srec`. Multi-language (EN/ZH). 

## STRUCTURE
- `en/`, `zh/`: Markdown source for English and Chinese documentation.
- `getting-started/`: Installation, first recording, and basic setup.
- `guides/` and `concepts/`: User workflows and explanations; existing `concepts/` URLs are retained for compatibility.
- `reference/`: Settings, environment variables, filename templates, processors, and workflow definitions.
- `operations/`: Deployment, storage, backup, monitoring, and security procedures.
- `development/`: Runtime architecture and implementation contracts for contributors.
- `.vitepress/`: VitePress engine, configuration, and theme.
  - `config.mts`: Central source of truth for sidebar, navbar, and locale routing.
- `public/`: Static assets (logos, diagrams, sample configuration files).

## WHERE TO LOOK
- `.vitepress/config.mts`: Update this for any sidebar or navigation changes.
- `en/getting-started/`: Core installation, Docker, and setup guides.
- `en/guides/`, `en/concepts/`: Recording, configuration inheritance, workflows, and notifications.
- `en/reference/`: Detailed field and processor lookup.
- `en/development/`: Internal service, persistence, and execution contracts.
- `public/`: Place images and shared assets here.

## CONVENTIONS / ANTI-PATTERNS
- **Sync**: Keep changed facts, interfaces, and navigation structure aligned between the corresponding `en/` and `zh/` pages. A wording or spelling fix in one language does not require a mechanical edit to the other or a full-site translation audit.
- **Toolchain**: Use Node from the repo-root `.nvmrc` and pnpm from this directory's `package.json`.
- **Sidebars**: Add new navigable pages to the appropriate locale's `sidebar` in `config.mts`. Auxiliary or intentionally unlisted pages need no sidebar entry.
- **Assets**: Reference assets in `public/` using root-relative paths (e.g., `/stream-rec.svg`).
- **Audience**: Keep implementation ownership, synchronization, and Rust APIs in `development/`. User guides retain observable behavior, configuration choices, failure recovery, and data-loss constraints. Describe current behavior; keep refactoring history in release notes.
- **Moved sections**: Update internal links to the new destination and preserve old fragment IDs. A `legacy-section` block is visible only when its ID is targeted; its link must lead to the exact replacement section.
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

## VALIDATION

For prose-only edits, check the affected facts and links. Run `pnpm run docs:build` for navigation/configuration changes, complex Markdown, or release-page preparation; preview affected pages when rendering or layout needs inspection. Report unavailable build prerequisites and remaining validation without blocking unrelated work. Docs changes do not require backend/frontend application builds.
