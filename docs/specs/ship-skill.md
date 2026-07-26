# ship-skill

## Goal

Ship a kicau agent skill (`skills/kicau/SKILL.md`) in the repo and have the
installer fetch it from the repo and drop it into the agent-framework skill
directories the user already has, so an assistant learns how to drive kicau. The
installer moves to `scripts/install.sh`.

## Non-goals

- **No creating framework dirs that do not exist.** A framework absent under
  `$HOME` is skipped, not created; `install.sh` never litters `$HOME`.
- **No bundling the skill in the release tarball.** The installer fetches it from
  the repo, so a skill or doc update ships without a new binary release.
- **No change to the kicau binary or its commands.**

## Acceptance criteria

- AC-1: `skills/kicau/SKILL.md` exists, with `name: kicau` and a trigger-phrased
  `description` in the frontmatter, and no em dashes.
- AC-2: `install.sh` fetches `SKILL.md` from the repo (`raw.githubusercontent.com
  /.../main/skills/kicau/SKILL.md`); the `release.yml` package step does not bundle
  it, so shipping the skill needs no new binary release.
- AC-3: given a fetched `SKILL.md`, `install.sh` copies it to
  `$HOME/<fw>/skills/kicau/SKILL.md` for each of `.agents`, `.claude`,
  `.openclaw`, `.hermes` whose `$HOME/<fw>` directory exists; a framework with no
  directory is skipped and is not created.
- AC-4: re-running the installer overwrites the skill, so updates propagate.
- AC-5: `install.sh` degrades gracefully when the skill fetch fails (offline, or
  the file absent): the skill step is skipped and the binary install still
  succeeds.
- AC-6: `install.sh`'s closing hint reads `kicau login`, not `kicau init`.
- AC-7: `sh -n scripts/install.sh` passes, and the copy logic is verified against
  a throwaway `$HOME` (installs to present frameworks, skips absent ones).
- AC-8: the installer lives at `scripts/install.sh`; the README curl command and
  the script's own header use the `.../main/scripts/install.sh` path, and no
  reference points at a root-level `install.sh`.
- AC-9: the skill documents kicau's full surface, and its MCP section states that
  the five existing tools are the whole MCP surface and the rest is CLI-only. The
  MCP tool set is unchanged (no tools added).

## Verification

sh -n scripts/install.sh
grep -q "name: kicau" skills/kicau/SKILL.md
! grep -q "—" skills/kicau/SKILL.md
grep -q "raw.githubusercontent.com.*skills/kicau/SKILL.md" scripts/install.sh
! grep -q "SKILL.md" .github/workflows/release.yml
grep -q "kicau login" scripts/install.sh && ! grep -q "kicau init" scripts/install.sh
# copy logic: with a temp HOME holding only some framework dirs, the skill lands
# in the present ones and the absent ones are not created.
