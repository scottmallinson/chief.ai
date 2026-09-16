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

### GHSA-wrw7-89jp-8q8g — `glib` 0.18.5 — moderate — accepted 2026-08-29, re-read 2026-09-16

**What it is.** Unsoundness in the `Iterator` and `DoubleEndedIterator` implementations for
`glib::VariantStrIter`. Fixed in `glib` 0.20.0.

**How it gets here.** Nothing in Chief depends on `glib`. It arrives through Tauri's Linux
webview stack and only there:

```
glib 0.18.5 ← gtk 0.18.2 / gdk / gio / webkit2gtk ← wry / tao ← tauri 2.11
```

`cargo tree -i glib` finds it for `x86_64-unknown-linux-gnu` and reports **nothing to print** for
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. Chief's own code never names `glib` and never
constructs a `VariantStrIter`.

**This advisory's original reasoning no longer holds, and this is the revisit it asked for.** It
was accepted on 2026-08-29 because Chief shipped macOS and Windows and no Linux bundle, so the
vulnerable crate reached no artifact anybody could install. **A Linux bundle was added on
2026-09-16**, which is one of the two triggers written into the line below. `glib` 0.18.5 is now
compiled into the Linux binary and shipped in all three of its packagings — the `.AppImage`, the
`.deb` and the `.rpm`. The macOS and Windows bundles are unaffected, and still reach it not at all.

**Why it is still not fixed — and it is genuinely not Chief's to fix.** `gtk` 0.18.2 requires
`glib ^0.18`, so `cargo update glib --precise 0.20.0` still fails to resolve. A fixed path does
exist upstream, which is a correction to what this entry said before: `gtk` 0.19.0 **is** published
and depends on `glib` 0.22, past the 0.20.0 that fixes this. Chief cannot take it, because it does
not depend on `gtk` directly — seven crates in Tauri's own stack pin `gtk` 0.18.2 (`tao`, `wry`,
`webkit2gtk`, `muda`, `tauri`, `tauri-runtime`, `tauri-runtime-wry`), and `cargo tree -i gtk` is
where that is read off. So the move is Tauri's to make. Dependabot is still right to raise the
alert and right not to offer a pull request — there is no version _Chief_ can bump to.

**What a Linux user is carrying.** Unsoundness in two iterator implementations in a crate Chief
never calls, reached only through the webview's own GTK3 bindings. It is not a remote-code-execution
primitive and there is no known exploit path through Tauri; it is accepted on those terms rather
than on the previous "it is not shipped at all", which was the stronger claim and is no longer
available.

**Revisit when** Tauri's stack moves to `gtk` 0.19 or off GTK3 — either now closes it, and the
first is newly plausible. Failing that, **2027-02-28**.
