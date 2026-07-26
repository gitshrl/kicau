# 12. Ship the agent skill from the repo, not the release

Status: Accepted

## Context

kicau ships `skills/kicau/SKILL.md`, a skill that teaches an agent kicau's
commands and its MCP tools. The question is how it reaches a user's machine.

Bundling it in the release tarball would tie every skill or doc edit to a new
binary build and release, even when the binary did not change. The binary and the
skill move at different speeds: the skill is prose that gets refined often.

## Decision

The skill lives in the repo and ships from `main`, not from the release.

- `scripts/install.sh` fetches `skills/kicau/SKILL.md` from `main` at install
  time and drops a copy into each agent framework already present under `$HOME`
  (`.agents`, `.claude`, `.openclaw`, `.hermes`). A framework with no directory is
  skipped, never created, so the installer does not litter `$HOME`.
- The release tarball carries only the binary. A skill or doc update ships the
  moment it lands on `main`, with no new release.
- The copy overwrites on every install, so a re-run picks up the latest skill.

## Consequences

- Editing the skill is a docs change, not a release.
- The installed skill tracks `main` rather than the pinned binary version. For a
  usage doc that is acceptable, and usually better: it is always current.
- A user offline at install time, or with none of the four frameworks present,
  gets no skill; the binary install still succeeds.
- The MCP surface stays the five existing tools; the skill states plainly that
  those five are the whole MCP surface and the rest is CLI-only, so an agent does
  not expect a tool that is not there.
