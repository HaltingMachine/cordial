# CLAUDE.md

@AGENTS.md

The above is the whole contract; everything below is Claude Code specifics.

## Subagents

Give a subagent the ADRs it needs by name, not "read the docs" — `docs/adr/` is
long and an agent that skims it will contradict a decision without noticing.

Tell it explicitly which files not to touch when work is in flight elsewhere.
Parallel agents in this repository have collided on `window.rs` and the native
shims more than once.

## Reporting back

The rule in AGENTS.md about never stating an unobserved result applies hardest
here, because a summary is where it slips. This project's history includes
claiming "no FastFlag reaches the engine", "~1 fps", and "stays up twelve
seconds" — all wrong, all measured with a broken instrument, all retracted in
later commits.

When a subagent reports success, that is a claim and not a result. Check it.
