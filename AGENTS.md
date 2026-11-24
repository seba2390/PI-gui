# Notes for agents

Read [CONTRIBUTING.md](CONTRIBUTING.md) first: ground rules, setup, and the checks to run before a
pull request.

## Releasing

Only when the user asks to release a version. It is one command:

1. Make sure what changed is under `## [Unreleased]` in `CHANGELOG.md` and committed. Issues close
   on the merge when a commit says `Fixes #<number>`.
2. Run `script/release.sh <version>` on the branch with the changes (allow about 10 minutes: it
   runs the tests). It opens the pull request and turns on auto-merge.
3. Stop. The merge, tag, builds, GitHub release and Homebrew update happen on GitHub, so don't
   wait for them, poll CI, merge, tag or publish by hand. Give the user the pull request's link and
   say that it is not released yet, and that a failed required check stops it.

If the script refuses, it says why: fix that and run it again.
