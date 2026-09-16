# Chief

A privacy-first, on-device AI chief of staff.

Chief answers questions about your work — what you shipped, what is waiting on you, what today looks
like — by reading your tools and reasoning with a local LLM. Everything happens on your machine:
no cloud backend, no remote model, no proxy server.

> **Status:** working software, released for macOS, Windows and Linux, and not finished. The app shell,
> the local database, the bundled LLM engine, the tool-calling agent, the background work-log
> daemon, the markdown corpus, drafted proposals, and integrations with GitHub, Linear, Jira,
> Confluence, Outlook and iCalendar subscriptions are all in. The roadmap beyond that is kept
> privately; the [issues list](https://github.com/scottmallinson/chief.ai/issues) is the public
> queue, and [CONTRIBUTING.md](CONTRIBUTING.md) says how to pick something up.

## Requirements

Chief ships installers for **macOS (Apple silicon and Intel) and Windows**. There is no Linux
bundle, but Linux is a perfectly good place to develop on — every quick check runs there, and the
frontend runs on its own in a browser with no Rust toolchain at all.

- [Node.js](https://nodejs.org) 20.19+ and [pnpm](https://pnpm.io) (`corepack enable`)
- [Rust](https://rustup.rs) 1.77.2+
- Linux only — the WebKitGTK toolchain:

  ```bash
  sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf xdg-utils
  ```

See the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for macOS and Windows.

## The engine

Chief runs [llama.cpp](https://github.com/ggml-org/llama.cpp) itself. `llama-server` is bundled into
the installer and started as a child process on a loopback port, so there is no separate runtime to
install and nothing left running once you close the app.

The binary is not in this repository — it is tens of megabytes of prebuilt CPU build, pinned to one
llama.cpp release. `pnpm engine:fetch` downloads the right one for your machine into
`src-tauri/binaries/`, and `pnpm tauri:dev` and `pnpm tauri:build` run it for you.

To work against a `llama-server` you are running yourself, set `CHIEF_LLAMA_BASE_URL` to its address
(loopback only) and Chief will use that instead of starting one.

## First run

Chief needs a model to run. The app checks for one when it starts and walks you through it: one
button downloads the weights with a progress bar, and the engine starts on them. No terminal
required, and an interrupted download resumes where it left off.

Connecting GitHub is one click in Settings: Chief shows a short code, opens your browser, and waits
while you authorise it. Your access token is stored in the local database and sent only to GitHub.

Running from source is the one case where you need your own OAuth app — see `.env.example`.

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
| `pnpm engine:fetch`             | Download the bundled llama.cpp server     |
| `pnpm check`                    | Format check, lint, typecheck and tests   |
| `pnpm fix`                      | Format and auto-fix what `check` flags    |
| `pnpm verify`                   | Everything CI runs, on this machine       |
| `pnpm test` / `pnpm test:watch` | Vitest                                    |
| `pnpm build`                    | Typecheck and build the frontend bundle   |
| `pnpm tauri build`              | Build installers for the current platform |
| `pnpm rust:lint`                | Clippy with warnings denied               |

## The work log

Once GitHub is connected, Chief periodically looks for pull requests you have merged, asks the local
model to turn each one into a single-sentence achievement, and records it. The summarising happens
on your machine, like everything else. Passes are idempotent, so the same merge is never logged
twice.

## How it stays private

- Inference runs in a llama.cpp server Chief starts on `http://127.0.0.1:11435`. The client refuses
  any address that is not on this machine, and the server is bound to loopback.
- The model weights are downloaded once, from Hugging Face, when you ask for them. That request
  carries no token, no cookie and nothing about you — it is the only thing in the app that talks to
  a host you did not connect yourself.
- Your work log, integration tokens and embeddings live in a local SQLite database in this app's
  config directory.
- Integrations authenticate from the app itself — GitHub's device flow, PKCE OAuth where the
  provider allows a public client, and a pasted key or subscription address where it does not.
  There is no server in between, and requests go straight from your machine to the service you
  connected.
- There is no telemetry, analytics or crash reporting.

## Contributing

Contributions are welcome — bug reports, fixes, integrations, documentation and the awkward
questions that turn into either. Start with **[CONTRIBUTING.md](CONTRIBUTING.md)**: setup, the one
command that runs everything CI runs, how commit types decide whether your change ships, and what
review looks for.

Two things worth knowing before you write code:

- **The privacy rule is not negotiable.** No hosted model, no relay server, no telemetry. A change
  that would send a user's data off their machine will not be accepted however good it is
  otherwise — see [the privacy bar](CONTRIBUTING.md#the-privacy-bar).
- **Open an issue first for anything larger than a fix.** The roadmap is kept privately, so a
  feature built in isolation may already be designed or deliberately deferred. Asking is cheap.

[CLAUDE.md](CLAUDE.md) is the long-form explanation of why the code looks the way it does — the
architecture, the conventions, and the reasoning behind most of the decisions that will look odd
from the outside. It is written for whoever works in the repository next, human or agent, and it is
the single best thing to read before a substantial change. [SECURITY.md](SECURITY.md) has the
private route for reporting a vulnerability, and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies
to everyone taking part.

## Licence

[MIT](LICENSE)
