#!/bin/sh
# Two commit-message guards, run from the `commit-msg` hook so a commit that
# would need amending never gets made. Amending is the documented fix, but a
# rebase over a branch of them is tedious and easy to get wrong, and a merged
# one cannot be corrected at all.
#
# The first guard applies to everybody, contributors included: a commit message
# describes the change, never which tool typed it.
#
# The second is opt-in, because this repository accepts outside contributions
# and a contributor commits under their own name. It is for a maintainer who
# wants the identity checked mechanically rather than remembered — most often
# because an agent is committing on their behalf and agents get this wrong.
# Turn it on per clone:
#
#   git config chief.commitIdentity 'you@example.com'
#
# Unset, the check is skipped and nothing here cares who you are.

set -e

message_file="$1"
status=0

# Attribution to a tool or a session, matched case-insensitively and anywhere in
# the message because it arrives in more shapes than the documented ones. A
# human co-author is fine and deliberately not matched — pair programming is
# not the thing being refused here.
tool_trailers='^[[:space:]]*(claude-session|generated (with|by)|assisted[- ]by)|^[[:space:]]*(co-authored-by|signed-off-by):.*(claude|anthropic|copilot|cursor|chatgpt|openai|gpt-[0-9]|gemini|codex|devin|aider)'

if grep -qiE "$tool_trailers" "$message_file"; then
	echo 'commit refused: the message credits a tool or a session for the work.' >&2
	echo >&2
	grep -inE "$tool_trailers" "$message_file" | sed 's/^/  /' >&2
	echo >&2
	echo 'CONTRIBUTING.md: the message describes the change, never who or what wrote it.' >&2
	echo 'A human co-author is fine; drop the tool trailer and commit again.' >&2
	status=1
fi

# The opt-in identity check. `git config` exits non-zero when the key is unset,
# which `set -e` would take as a failure, so the miss is absorbed.
expected=$(git config chief.commitIdentity 2> /dev/null || true)

if [ -n "$expected" ]; then
	for role in AUTHOR COMMITTER; do
		identity=$(git var "GIT_${role}_IDENT")
		email=$(printf '%s' "$identity" | sed -n 's/.*<\(.*\)>.*/\1/p')

		if [ "$email" != "$expected" ]; then
			echo "commit refused: ${role} is <${email}>, not <${expected}>." >&2
			wrong_identity=1
			status=1
		fi
	done

	if [ "${wrong_identity:-0}" -eq 1 ]; then
		echo >&2
		echo "This clone has chief.commitIdentity set to <${expected}>. Either set the" >&2
		echo 'identity to match:' >&2
		echo >&2
		echo "  git config user.email '${expected}'" >&2
		echo >&2
		echo 'or clear the expectation, if this clone is not meant to hold it:' >&2
		echo >&2
		echo '  git config --unset chief.commitIdentity' >&2
	fi
fi

exit "$status"
