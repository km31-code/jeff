# Jeff — Claude Code Operating Instructions

## Always do first, every session
1. Read docs/VISION.md in full. This is the product definition. 
   All technical decisions must serve it.
2. Read docs/PHASES.md. Know which phase is current.
3. Read docs/ARCHITECTURE.md for the current layering.

## Workflow rules (non-negotiable)
- Verification-first. Never claim a milestone is complete without:
  - running the relevant phase check script (scripts/phaseN_check.sh)
  - runtime proof (app launches, feature works end to end)
  - explicit scope boundary — what is in this milestone, what is not
- Work one milestone at a time. Do not combine milestones.
- Show the plan before writing code. Propose the exact changes and
  wait for explicit approval before editing.
- Preserve existing code paths. Add minimal fallbacks or guards 
  rather than deleting features or doing large rewrites, unless 
  the current phase plan explicitly calls for a rewrite.
- Do not expand backend capability beyond the current phase's scope.

## Code style
- No emojis anywhere. Not in UI copy, not in comments, not in commit 
  messages, not in chat output.
- All code comments in lowercase.
- TypeScript for frontend, Rust for backend. Follow existing patterns.

## Output style
- Structured and actionable. No filler.
- When you finish a milestone, state: what changed, what was verified,
  what is explicitly out of scope, what the next milestone is.

## When unsure
- Ask before assuming. Never silently broaden scope.
- If a milestone reveals the plan is wrong, stop and surface it.
  Do not "work around" a bad plan.