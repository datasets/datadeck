# Agent instructions

Keep project tasks in Beads. Run `bd prime` for the current workflow and repository guidance. See [Beads sync](docs/beads-sync.md) for setup and recovery.

At session start, run `git pull`, `bd dolt pull`, and `bd status`. At session end, commit and push Git changes. The pre-push hook also pushes Beads Dolt data; run `bd dolt push` manually if it reports a sync failure.
