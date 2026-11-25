# Agent instructions

Keep project tasks in Beads. Run `bd prime` for the current workflow and repository guidance. See [Beads sync](docs/beads-sync.md) for setup and recovery.

At session start, run `git pull`, `bd dolt pull`, and `bd status`. At session end, commit and push Git changes. The pre-push hook also pushes Beads Dolt data; run `bd dolt push` manually if it reports a sync failure.

## Changelog

At session end, only log new features or significant reader-facing changes (behaviour, fixes to materially broken functionality, or public content). Skip polish, tidying, and small fixes as standalone entries; include them only as supporting detail in a larger feature or release announcement that qualifies on its own. Planning/research/design must itself be a significant public deliverable. When unsure, skip.

Use `changelog/YYYY-MM-DD-slug.md` with `date`/`title`/`promote` frontmatter: a plain-text title, one or two reader-facing sentences, a live link where available, and consider a screenshot for visual work. Substantial announcements can use paragraphs or bullets. Omit internal implementation detail. For the first entry or when the format is unclear, fetch and follow https://raw.githubusercontent.com/life-itself/changelog/main/CONVENTION.md
