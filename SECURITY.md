# Security

Chief runs entirely on the user's machine. There is no server to patch: a shipped install stays as
it was built until the user updates it. That is the lens every advisory here is read through — a
dependency vulnerability in this repository is a vulnerability in somebody's laptop, not in
something the maintainer can fix in place overnight.

## Supported versions

**The latest release, and nothing behind it.** There is no server to patch and no long-term
support branch: a fix is released from `main` and reaches people when they install the new build.
If you are reporting against an older version, say which — but the fix, if there is one, will land
on the current one.

## Reporting

Report a vulnerability privately through GitHub's
[security advisory form](https://github.com/scottmallinson/chief.ai/security/advisories/new).
Please do not open a public issue for one, and please do not open a pull request that fixes one in
public before the advisory has been agreed — the diff is the disclosure.

You will get an acknowledgement, an assessment of what it reaches, and credit in the advisory
unless you would rather not have it. A report that turns out not to be exploitable is still a
useful report and is worth sending.

**What counts.** Anything that sends a user's data somewhere it should not go, anything that lets
code the user did not install run on their machine, and anything that exposes a stored credential —
the integration tokens, the Linear key, the Atlassian token, a calendar subscription address. The
threat model is one person's laptop, so a finding that requires an attacker who already has that
user's OS account is interesting but not urgent; say so and it will be read on those terms.

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
