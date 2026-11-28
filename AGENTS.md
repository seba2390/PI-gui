# Notes for agents

Read [CONTRIBUTING.md](CONTRIBUTING.md) first: ground rules, setup, and the checks to run before a
pull request.

## Keep `target/` small

Build output piles up (debug, release and test builds, stale incremental caches) and has passed
90 GB. When a task builds or tests, check its size first with `du -sh target`, in every checkout
and worktree you build in (each has its own `target/`). Above 30 GB, run `cargo clean` before
building, and say so: the next build starts from scratch (a few minutes in release). Never clean
while another build or test in that checkout is running.

## Releasing

Only when the user asks to release a version. After a documentation review, it is one command:

1. Make sure what changed is under `## [Unreleased]` in `CHANGELOG.md` and committed. Issues close
   on the merge when a commit says `Fixes #<number>`.
2. Review the documentation, every release, even a patch or internal-only one and even when no
   documentation file changed. Follow "Review the documentation before every release" under
   Releasing in [CONTRIBUTING.md](CONTRIBUTING.md): learn what changed from the commit messages
   since the last tag, the changelog and pull requests' descriptions (not by reading every diff:
   inspect code only for a specific open question), go through all the documentation, fix and
   commit what is stale, missing or contradictory, build the site when its content changed, and
   write the summary to a file outside the repository. If something stays unclear or validation
   fails, stop and tell the user instead of releasing.
3. Run `script/release.sh <version> --docs-review <file>` on the branch with the changes (allow
   about 10 minutes: it runs the tests). It opens the pull request, with the review in it, and
   turns on auto-merge.
4. Stop. The merge, tag, builds, GitHub release and Homebrew update happen on GitHub, so don't
   wait for them, poll CI, merge, tag or publish by hand. Give the user the pull request's link and
   say that it is not released yet, and that a failed required check stops it.

If the script refuses, it says why: fix that and run it again.
