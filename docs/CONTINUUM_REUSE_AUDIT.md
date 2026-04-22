# Continuum Reuse Audit (Phase 0)

Phase 0 policy: inspect Continuum for reference only, avoid importing large systems.

## Good Early Reuse Candidates (small, low-coupling)

- utility-level patterns for structured logging
- data validation helpers or small shared types
- test harness ideas and script patterns
- ergonomic error-handling conventions

## Possible Later Reuse Candidates (after Jeff core is stable)

- selected persistence helpers once Jeff task schema exists
- selected retrieval primitives once Jeff retrieval architecture is defined
- stable protocol/contract shapes if they map directly to Jeff requirements

## Must Not Be Reused in Phase 0

- compositor/surface stack
- OS-level capability/security model layers
- full daemon/runtime orchestration subsystems
- automation-heavy modules not aligned with Jeff v1 scope

## Decision Rule

Reuse only small, well-bounded components that reduce risk without importing Continuum architectural assumptions.
