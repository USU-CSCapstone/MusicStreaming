# 0005. One data directory split into `state/` and `cache/`

- **Date:** 2026-09-25 12:05 -0600
- **Status:** Accepted
- **Commit:** eed6e3d (merged in d16f9a4)

## Context
The music library is read-only to Jewelcase, so all of its own state needs one home that is easy to back up.

## Decision
The server writes only under its data directory:
- `state/` holds precious data, including the database, which stores analysis results as well.
- `cache/` holds anything that can be rebuilt.

## Consequences
- Backing up means copying `state/`.
- Anything that is expensive to rebuild must go in `state/`, even if it could in principle be regenerated.
