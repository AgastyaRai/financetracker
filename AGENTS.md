# FinanceTracker Coding and Review Guide

This file contains project-specific instructions for coding agents working in this repository.

## Non-negotiable preservation rules

- Never remove existing comments or documentation. Do not rewrite or relocate them unless the project owner explicitly approves it.
- Preserve the existing code, naming, formatting, file organization, and control-flow style wherever practical.
- Avoid drive-by formatting, renaming, warning cleanup, or unrelated refactoring.
- Treat user-authored code and uncommitted changes as intentional unless the task explicitly says otherwise.

## Approval and scope

- Explain and obtain clarification before making a serious functionality, API, schema, dependency, or architecture change.
- Readability-only refactors must preserve behavior and remain narrowly focused on the code being discussed.
- State which files and behaviors a proposed change will affect before implementing it.

## Development workflow

- Work test-first: add or identify a focused behavioral test, confirm the failure, implement the smallest change, then run the focused and full regression suites.
- Use fake providers for automated embedding tests. Tests that call the real OpenAI API must remain explicit and ignored by default.
- Use disposable test databases for destructive database testing. Do not modify or recreate the normal local database without permission.
- Do not commit or push changes unless the project owner asks for it.

## Readability standards

- Keep HTTP handlers focused on request validation and orchestration. Extract provider integration, complex error handling, and reusable database behavior into clearly named helpers.
- Prefer typed SQLx result structures with `query_as` and `FromRow` over repeated `Row::get` calls for multi-column records.
- Use small domain structures when several related values repeatedly travel together.
- Prefer early returns over deeply nested `match` or `if` blocks.
- Use concise imports when repeated fully qualified type names obscure the main control flow.
- Comments must explain intent, invariants, ordering, failure behavior, or concurrency guarantees. Do not limit comments to restating the next line of code.
- Complex database/provider workflows should explain why transaction boundaries and external calls occur in their chosen order.

## Review checklist

- Confirm no existing comment or documentation line was removed.
- Confirm the diff contains no unrelated formatting or structural changes.
- Confirm ownership checks are enforced in database queries for user-owned records.
- Confirm financial writes are not rolled back by optional external-provider failures.
- Confirm delayed provider responses cannot overwrite newer transaction state.
- Run `git diff --check`, focused tests, and the full backend test suite before handing work back for review.
