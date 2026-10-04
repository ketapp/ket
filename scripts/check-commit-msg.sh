#!/bin/sh
# Checks one commit subject or pull request title against the format in
# CONTRIBUTING.md:
#
#   type(scope): what changed (#issue)
#
# `feat` and `fix` name the issue they close; the other types may. Used by
# `.githooks/commit-msg` and by the PR title check in CI, so both refuse the
# same things.

subject=$1
types='feat|fix|perf|refactor|docs|chore|ci|build|revert'

case "$subject" in
'Merge '* | 'Revert "'* | 'fixup! '* | 'squash! '* | 'amend! '*) exit 0 ;;
esac

fail() {
	printf '%s\n' "commit format: $1" "  got:      $subject" >&2
	printf '%s\n' "  expected: type(scope): what changed (#issue)" \
		"  types:    ${types}" "  e.g.      fix(editor): triple-click selects the line (#142)" >&2
	exit 1
}

printf '%s\n' "$subject" | grep -Eq "^($types)(\([a-z0-9-]+\))?!?: [^ ]" ||
	fail "no type, or no ': ' after it"

if printf '%s\n' "$subject" | grep -Eq '^(feat|fix)[(!:]'; then
	printf '%s\n' "$subject" | grep -Eq ' (\(#[0-9]+\)|#[0-9]+)$' ||
		fail "feat and fix end with the issue they close, as (#142)"
fi
