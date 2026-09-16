## Summary

<!-- What does this change do, and why? -->

## Related issue

<!--
Closes #123, or "none — self-contained fix".

Anything larger than a fix is worth an issue first: the roadmap is kept privately, so a reviewer may
know something about the shape of this that you could not. See CONTRIBUTING.md.
-->

## How I tested

<!--
The commands you ran, plus anything you exercised by hand in the app.

If this change adds a test that guards an invariant — a cap, a boundary, a "never overwrites", a
"no network call" — say what you broke to watch it fail, and paste the failure message. Those tests
pass on the day they are written whatever they assert.
-->

- [ ] `pnpm verify` — or the jobs that cover what I touched
- [ ] Exercised in the running app, where the change is visible there

## Privacy checklist

- [ ] No new outbound network call beyond `localhost`, a service the user explicitly connected, or the model-weights download
- [ ] No secrets, tokens or user data added to the repository
- [ ] New persistence stays in the local SQLite database, behind a typed command

## For the maintainer

<!-- Ignore this section if you are not the maintainer. -->

- [ ] The plan's §0 rows this moves are named above
