#!/bin/sh
# The old name of scripts/package.sh, kept so that it still works.
exec "$(dirname "$0")/package.sh" "$@"
