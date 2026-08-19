# Chief

A privacy-first, on-device AI chief of staff.

Chief answers questions about your work — what you shipped, what is waiting on you, what today looks
like — by reading your tools and reasoning with a local LLM. Everything happens on your machine:
no cloud backend, no remote model, no proxy server.

> **Status:** early development. Steps 1–4 of the roadmap are in place: the app shell, the local
> SQLite database, the local LLM engine and the tool-calling orchestrator. `fetch_github_prs` still
> answers with sample data — connecting a real GitHub account is the next step.

## Requirements

- [Node.js](https://nodejs.org) 20.19+ and [pnpm](https://pnpm.io) 10 (`corepack enable`)
- [Rust](https://rustup.rs) 1.77.2+
- [Ollama](https://ollama.com) running locally, with the default model pulled:

  ```bash
  ollama serve
  ollama pull llama3.2:3b
  ```

- Linux only — the WebKitGTK toolchain:

  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
  ```

See the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for macOS and Windows.

## Getting started

```bash
pnpm install
pnpm tauri:dev
```

`pnpm dev` runs the frontend on its own at http://localhost:1420, which is handy for pure UI work.

## Scripts

| Command                         | What it does                              |
| ------------------------------- | ----------------------------------------- |
| `pnpm tauri:dev`                | Run the desktop app with hot reload       |
| `pnpm check`                    | Format check, lint, typecheck and tests   |
| `pnpm test` / `pnpm test:watch` | Vitest                                    |
| `pnpm build`                    | Typecheck and build the frontend bundle   |
| `pnpm tauri build`              | Build installers for the current platform |
| `pnpm rust:lint`                | Clippy with warnings denied               |

## How it stays private

- Inference runs against Ollama on `http://localhost:11434`.
- Your work log, integration tokens and embeddings live in a local SQLite database in this app's
  config directory.
- Integrations authenticate with PKCE OAuth directly from the app — there is no server in between,
  and requests go straight from your machine to the service you connected.
- There is no telemetry, analytics or crash reporting.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Architecture notes and conventions live in
[CLAUDE.md](CLAUDE.md).

## Licence

[MIT](LICENSE)
