# Repository Guidelines

## Project Structure & Module Organization

This desktop file manager uses Vue 3, TypeScript, Vite, and Tauri 2. Frontend code is in `src/`: `components/` for UI, `api/` and `shell/` for native bridges, `stores/` for Pinia state, and `styles/` and `assets/` for presentation resources. Rust code and Tauri configuration are in `src-tauri/`, with native modules under `src-tauri/src/`. Static web assets are in `public/`. Treat `dist/` and `src-tauri/target/` as generated output.

## Build, Test, and Development Commands

Use pnpm; the repository tracks `pnpm-lock.yaml`:

- `pnpm install` installs frontend dependencies.
- `pnpm dev` starts the Vite frontend.
- `pnpm tauri dev` runs the desktop application.
- `pnpm build` type-checks with `vue-tsc` and builds with Vite.
- `pnpm tauri build` packages the desktop application.
- `cargo test --manifest-path src-tauri/Cargo.toml` runs Rust tests when present.

There is no frontend test or lint script currently.

## Coding Style & Naming Conventions

Follow existing Vue Composition API patterns and use `<script setup lang="ts">` in single-file components. Use two spaces in TypeScript, Vue, JSON, and CSS; format Rust with `cargo fmt`. Name Vue components in PascalCase (for example, `FileList.vue`), utilities in camelCase, and Rust modules in snake_case. Keep filesystem operations in Rust and use the existing frontend bridge.

## Testing Guidelines

Place tests near the behavior they cover. Rust tests can live alongside modules in `src-tauri/src/`; run them with the command above. For frontend changes, run `pnpm build` to catch Vue and TypeScript errors. Exercise filesystem operations in the Tauri app using safe temporary folders.

## Commit & Pull Request Guidelines

Recent commits use concise imperative subjects, often with prefixes such as `feat(search): ...` and `fix(capabilities): ...`; Chinese subjects are also used. Prefer `type(scope): short description`. Pull requests should describe user-visible changes, link related issues, include screenshots for visual changes, and list validation commands and platform limits.

## Security & Configuration Tips

Do not commit API keys, personal paths, or machine-specific configuration. Review `src-tauri/capabilities/` when adding native commands or plugin access, and grant only required permissions.
