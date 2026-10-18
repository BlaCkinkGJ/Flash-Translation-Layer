# AI work instructions

**Scope: the orchestrating agent only.** A sub agent must not follow this
document. Sub agents follow the single task brief they are handed, and nothing
here applies to them.

## Goal

Finish the Rust migration of the FTL and keep going until no open issue carries
the `rust-migration` label.

## Order of work

1. Land the pull requests that are already open before starting new work.
2. Then take the remaining issues in dependency order.
3. Run issues that touch disjoint files in parallel, each in its own git
   worktree with its own sub agent, so that no two agents edit the same file.
4. When a pull request merges, tell every agent that is still working to rebase
   onto the new `main` before it finishes.
5. Add a `rust-migration` issue the moment the work uncovers one, then fold it
   into the plan. The job is done when none are left open.

## Per issue

1. Read the issue and the C source it names. The issue body is the binding
   spec, not a summary of it.
2. Implement against the C original, then compare the two behaviours before
   calling the port done: return values, error codes, ownership and free
   ordering, allocation failure paths, null and out of range inputs, struct
   layout, and the counter updates each side performs.
3. CI/CD must pass on the pull request.
4. Line coverage of the changed lines must reach 90 per cent. Measure it, never
   assume it.
5. Open the pull request with `Closes #N` and the title
   `[rust-migration] Task N: <issue title>`.
6. Review it with at least three review agents, running `/skill:code-review`
   and `/skill:ponytail-review` on separate axes. Fix what they find and repeat
   until a round turns up no request for a meaningful change. Skip style taste,
   and skip anything tooling already enforces.
7. Merge once the checks are green, the findings are settled, and the coverage
   gate holds. Then run step 4 of the order of work.

## Standing rules

- The C implementation stays linked and untouched until the cutover task. Every
  new C ABI symbol carries the `ffi_` prefix.
- No semicolons and no em dashes in prose, comments, or commit messages.
- Never force-push `main`, never bypass branch protection, never merge a pull
  request whose checks are red.
