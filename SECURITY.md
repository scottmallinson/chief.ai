# Security

Chief runs entirely on the user's machine. There is no server to patch: a shipped install stays as
it was built until the user updates it. That is the lens every advisory here is read through — a
dependency vulnerability in this repository is a vulnerability in somebody's laptop, not in
something the maintainer can fix in place overnight.

## Reporting

Report a vulnerability privately through GitHub's
[security advisory form](https://github.com/scottmallinson/chief.ai/security/advisories/new).
Please do not open a public issue for one.

## Accepted advisories

Dependabot raises alerts against `pnpm-lock.yaml` and `src-tauri/Cargo.lock`. Most are fixed by a
bump. The ones below cannot be, and are accepted deliberately rather than left unread. Each carries
the reason and a date to look again, so nobody has to re-derive the analysis.

### GHSA-wrw7-89jp-8q8g — `glib` 0.18.5 — moderate — accepted 2026-08-29

**What it is.** Unsoundness in the `Iterator` and `DoubleEndedIterator` implementations for
`glib::VariantStrIter`. Fixed in `glib` 0.20.0.

**How it gets here.** Nothing in Chief depends on `glib`. It arrives through Tauri's Linux
webview stack and only there:

```
glib 0.18.5 ← gtk 0.18.2 / gdk / gio / webkit2gtk ← wry / tao ← tauri 2.11
```

`cargo tree -i glib` finds it for `x86_64-unknown-linux-gnu` and reports **nothing to print** for
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. Chief ships macOS Apple silicon, macOS Intel
and Windows, and no Linux bundle — so the vulnerable crate is not compiled into, or shipped in, any
artifact a user can install. Chief's own code never names `glib` and never constructs a
`VariantStrIter`.

**Why it is not fixed.** `gtk` 0.18.2 requires `glib ^0.18`, so `cargo update glib --precise 0.20.0`
fails to resolve. There is no `gtk` 0.19 or 0.20 to move to: the GTK3 bindings are finished and
marked unmaintained upstream, with `gtk4` as the successor. No release of Tauri v2 changes this,
because Tauri v2 renders through WebKitGTK, which is a GTK3 library. Dependabot is right to raise
the alert and right not to offer a pull request — there is no version to bump to.

**Revisit when** Tauri's Linux backend moves off GTK3, or a Linux bundle is added to the release —
whichever comes first. Failing either, **2027-02-28**.
