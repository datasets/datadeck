# Beads sync

This repo tracks Beads configuration and hooks in Git. Issue data lives in the local Dolt database and syncs through `git+ssh://git@github.com/datasets/datadeck.git`. JSONL export is disabled.

## Normal workflow

At the start of a session, run `git pull`, `bd dolt pull`, and `bd status`. Track tasks with `bd create`, `bd update`, and `bd close`. At the end, commit changes and run `git push`; the pre-push hook also runs `bd dolt push`. Run `bd dolt push` directly for Beads-only changes or if the hook reports a failure.

## Fresh clone

Clone with `git clone git@github.com:datasets/datadeck.git`. Run `git config core.hooksPath .beads/hooks`, then `bd bootstrap`, `bd dolt pull`, and `bd status`. If `bd bootstrap` finds a ready local database, continue with `bd dolt pull`.

## Sync failure

Inspect `bd context`, `bd dolt status`, `bd dolt remote list`, and `git status --short --branch`. Retry `bd dolt pull` before pushing. Preserve both histories if they diverge; follow the [shared Beads sync playbook](https://github.com/rufuspollock/skills/blob/main/beads-sync-playbook.md) before any force operation.

## Setup recipe

Use `bd init --non-interactive --prefix datadeck --role maintainer --skip-agents`, set `export.auto` to `false`, and configure the `origin` Dolt remote as `git+ssh://git@github.com/datasets/datadeck.git` with `--allow-git-origin`. Keep local database and runtime files ignored. Configure Git hooks with `git config core.hooksPath .beads/hooks`. The tracked hook additions push Dolt on Git push and pull it after merges and branch checkouts.
