# Contributing

## Setup

```bash
corepack enable
pnpm install
pnpm tauri:dev
```

Platform prerequisites are listed in the [README](README.md#requirements).

## Before you push

```bash
pnpm check                    # format, lint, typecheck, test
pnpm rust:fmt && pnpm rust:lint && pnpm rust:test
pnpm tauri build --no-bundle  # if you touched Rust or Tauri config
```

A pre-commit hook runs `lint-staged` (ESLint, Prettier and `rustfmt` on staged files), and a
`commit-msg` hook validates your commit message.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>
```

- **Types:** `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`,
  `revert`
- **Scopes:** `agent`, `auth`, `db`, `daemon`, `integrations`, `ui`, `tauri`, `deps`, `ci`, `repo`

Examples:

```
feat(db): add work_logs and integrations migrations
fix(agent): keep tool call ids stable across the response loop
chore(deps): bump tauri to 2.2.7
```

Keep each commit to one logical change. Pull requests are squash-merged, so the PR title must also
be a valid conventional commit — CI checks both.

## Pull requests

- Branch off `main`.
- Fill in the PR template, including the privacy checklist.
- Keep a PR to a single roadmap step where possible.
- CI compiles the app on Linux only for a pull request; macOS and Windows build on every push to
  `main`. If your change is likely to land differently there — anything touching the webview, the
  bundle or a platform path — run the CI workflow against your branch from the Actions tab, which
  builds all three.

## The privacy bar

Any change that would send user data off the device — a hosted model, a relay server, telemetry, an
error reporter — will not be accepted. Network access belongs in the Rust backend and is limited to
`localhost` and services the user has explicitly connected.
