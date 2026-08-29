#!/bin/sh
# Everything this repository publishes is the owner's work, whoever or whatever
# typed it. CLAUDE.md says so; this makes it mechanical rather than remembered.
#
# Run from the commit-msg hook, so a commit that would need amending never gets
# made. Amending is the documented fix, but a rebase over a branch of them is
# tedious and easy to get wrong, and a merged one cannot be corrected at all.

set -e

message_file="$1"
owner_email='scott@scottmallinson.com'
status=0

# Trailers and bylines that attribute the work to a tool or a session. Matched
# case-insensitively and anywhere in the message, because they arrive in more
# shapes than the documented ones.
if grep -qiE '^[[:space:]]*(co-authored-by|claude-session|generated (with|by)|assisted[- ]by|signed-off-by:.*claude)' "$message_file"; then
	echo 'commit refused: the message carries a tool or co-author attribution.' >&2
	echo >&2
	grep -inE '^[[:space:]]*(co-authored-by|claude-session|generated (with|by)|assisted[- ]by|signed-off-by:.*claude)' "$message_file" | sed 's/^/  /' >&2
	echo >&2
	echo 'CLAUDE.md: the message describes the change, never who or what wrote it.' >&2
	status=1
fi

# The author and the committer are both the owner. An agent that forgot to set
# these leaves a commit that has to be amended with --reset-author, and this is
# the last moment where that is free.
for role in AUTHOR COMMITTER; do
	identity=$(git var "GIT_${role}_IDENT")
	email=$(printf '%s' "$identity" | sed -n 's/.*<\(.*\)>.*/\1/p')

	if [ "$email" != "$owner_email" ]; then
		echo "commit refused: ${role} is <${email}>, not <${owner_email}>." >&2
		wrong_identity=1
		status=1
	fi
done

if [ "${wrong_identity:-0}" -eq 1 ]; then
	echo >&2
	echo 'Fix the identity with:' >&2
	echo "  git config user.name 'Scott Mallinson'" >&2
	echo "  git config user.email '${owner_email}'" >&2
fi

exit "$status"
