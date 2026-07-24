---
name: phoenix
description: >-
  Builds and extends websites with Phoenix-rs (Rust + React, px CLI,
  Laravel-style routes/controllers, Toasty, typed contracts, @phoenix/react).
  Use when creating a Phoenix-rs app, scaffolding with px, writing controllers/
  routes/migrations/pages, or when the user mentions Phoenix-rs, Phoenix, px new,
  px make, PageEnvelope, callRust, or Laravel-like Rust web apps.
---

# Phoenix-rs Framework Skill

Phoenix-rs = Hyper HTTP + Laravel-style DX + React (Islands/SPA/SSR) + Rust→TS contracts.
Not a Laravel port; not Elixir Phoenix. Prefer `phoenix::prelude::*` and generated `views/generated/*`.
Install CLI: `cargo install px-cli` (binary is `px`; or `cargo install --path crates/phoenix-cli` from this repo).

**Framework repo docs (source of truth):** `docs/BUSINESS_GUIDE.md`, `docs/DX.md`, `docs/CONFIG.md`, `docs/FEATURES.md` (Cargo features + plugins), `docs/RELEASE_PIPELINE.md`, `docs/CONTRACTS.md`, `docs/RENDERING.md`, `docs/REACT_DX_*.md`, `docs/工具与约定.md`.

## When this skill applies

**Default for this repository:** any coding task under Phoenix / `px` / `@phoenix/react` MUST load this skill first (see root `AGENTS.md` and README 「AI / Agent 开发」).

Also use when:

- New app / feature in Phoenix
- `px new` / `px make:*` / `px migrate` / `px dev`
- Routes, controllers, Request DTO, Resource, Page props, React pages/islands
- `@phoenix/react` navigation, forms, hooks

## Non-negotiables

1. **Scaffold with CLI**, do not hand-roll project trees when `px` exists.
2. **Contracts live in Rust** (`#[phoenix::contract(...)]`). Never hand-edit `views/generated/`.
3. **Named routes** everywhere; React uses generated `routes.ts` / actions, not hardcoded URLs (except page-protocol `PageForm` action paths when intentional).
4. **Security defaults on**: Session/CSRF, Nonce CSP in scaffolds; do not strip for convenience.
5. **DELETE/PUT mutations** go through typed JSON actions — no method-spoofing Links.
6. After structural changes: `cargo test` / `cargo check` and frontend `tsc`/tests as appropriate.

## New project checklist

```text
Task Progress:
- [ ] px new <app> && cd <app>
- [ ] px make:model <Name> --all  (or smaller make:* steps)
- [ ] Edit migration SQL + model fields + validation rules
- [ ] Wire queries in controller; keep Page props / Resource contracts accurate
- [ ] Implement views/pages + islands; use generated types/actions
- [ ] px migrate (or app console migrate path)
- [ ] px dev — verify page + action + 422 errors
- [ ] Add feature tests under tests/feature/
```

### Commands

```bash
px new my-app
cd my-app
cp .env.example .env
# Enable Cargo features as needed, e.g. --features sqlite,password
# Align database.toml default with the enabled driver feature when using DB.
# Optional: tls / websocket / sse / auth / jwt / metrics (see docs/FEATURES.md)
px make:model Post --all
px make:controller Admin/PostController --resource
px make:page posts/index
px make:island LikeButton
px make:command Update
px migrate
px dev
# ship:
px release --version 0.1.0 --tarball
# on server: px release:install --tarball … --version 0.1.0
```

App binary is console-style: `cargo run -- serve` / `cargo run -- <command>`.

### Directory map

```text
app/controllers|middleware|models|requests|resources|commands|props/
routes/*.rs          # mount_routes!()
database/migrations|seeders/
views/pages|islands|components|layouts|generated/   # generated = DO NOT EDIT
config/app.toml|database.toml|schemas/  taplo.toml
deploy/restart.sh.example
public/  storage/  dist/  tests/feature|unit/
```

## Feature workflow (existing app)

1. Prefer `px make:*` for new artifacts (registers mod/routes/contracts).
2. Routes: `.name("...")` + `.action::<Input, Output>()` for callable TS actions.
3. Controllers: async fn / typed extractors (`Json`, `Validated`, `State`, `Path`, …).
4. Pages: return `Page` / page envelope; React page under `views/pages/...`.
5. Islands: `client:load` in TSX; no manual island registry.
6. Forms: JSON action `Form`/`useForm` **or** page-protocol `PageForm`; use `form.field` + `*Fields` from contracts.
7. Third-party Feature: implement `Plugin`, `FeatureSet::new().plugin(...)`, merge routes/commands (see `docs/FEATURES.md`).
8. Ship: `px release` → upload → `px release:install` (see `docs/RELEASE_PIPELINE.md`).

## Architecture cheat sheet

| Layer | Choice |
| --- | --- |
| HTTP | Hyper 1.x via Phoenix-rs |
| ORM | Toasty 0.8 (SQLite / PostgreSQL / MySQL) |
| Config | `config/*.toml` + `.env` (Taplo schema) |
| Frontend | Vite + React + TS; default Islands |
| Contracts | Rust sole source → `contracts.ts` + `routes.ts` |
| Plugins | Compile-time `Plugin` / `FeatureSet` |
| Release | `phoenix-release` + `px release*` |
| Session/Redis/Queue/Mail | Features; Memory backends for local; Redis via feature |

Optional Cargo features on `phoenix`: `redis`, `storage`, `testing`, `queue`, `mail`.

## Decision tree

**Need a full CRUD resource?** → `px make:model X --all`, then fill SQL + controller logic.

**Need JSON API only?** → route + `.action::<In, Out>()` + Resource contract; no page required.

**Need a React page?** → controller returns page props contract + `views/pages/...tsx`.

**Need client mutation without full navigation?** → typed action + `Form` / `useForm` / `useOptimisticAction`.

**Need submit then new page (Inertia-like)?** → `PageForm` with `method` + visit options.

**Need background work / email?** → `phoenix-queue` / `phoenix-mail` (Memory first; real SMTP later).

## Anti-patterns

- Editing `views/generated/*`
- Duplicating TS interfaces that exist as Rust contracts
- Hardcoding `/api/...` in React when a named action exists
- Skipping CSRF on cookie-session POSTs
- Inventing a second DI container — use `State<T>` / explicit constructors
- Method-spoofing `<Link method="delete">`
- Blindly copying Laravel Facades / Eloquent magic that Toasty does not provide

## Progressive disclosure

- Rust API patterns → [api-rust.md](api-rust.md)
- React / `@phoenix/react` → [api-react.md](api-react.md)
- Full narrative guides → framework `docs/BUSINESS_GUIDE.md`, `docs/DX.md`
