**English** | [简体中文](CONTRIBUTING.zh-CN.md)

# Contributing to gilvt

Thanks for your interest. gilvt is maintained by one person in their spare time, so the process below is
designed to keep reviews small and predictable.

Before contributing, read the [AI usage policy](AI_POLICY.md).

## Quick guide

**I found a bug.** Open an issue with the bug report template. Include the gilvt version or commit, your
macOS version, and the Claude Code / Codex versions if an agent is involved. Many problems come from a
change in an agent CLI's output format, so those versions matter.

**I have an idea for a feature.** Open a feature request issue and describe the problem first, then your
proposal. Please wait for a maintainer to agree before writing code.

**I want to fix or implement something.** Pick an issue that a maintainer has accepted (or open one),
say you are working on it, then send a pull request that references it. Pull requests without an
accepted issue may be closed, except for obvious small fixes (typos, broken links, a one-line bug fix).

**I found a security problem.** Don't open an issue; follow [SECURITY.md](SECURITY.md).

## Development

Build and code layout: [HACKING.md](HACKING.md) (currently in Chinese).

```bash
cargo build --workspace
cargo test --workspace
```

Use `cargo build --workspace` rather than `cargo run -p gilvt-app`: the `gilvt` CLI must be built next to
`gilvt-app`. Use `scripts/bundle.sh` to produce `Gilvt.app` when you need notifications or the Dock.

### Code style

- Match the surrounding code: naming, comment density and line length. The codebase is not formatted
  with `rustfmt` yet, so don't reformat files you aren't otherwise changing.
- No new compiler warnings.
- Keep crates that are documented as gpui-free (`gilvt-term`, `gilvt-agent`, `gilvt-editor`, …) free of
  gpui; see the table in HACKING.md.

### Tests and acceptance cases

- Add unit or integration tests for logic changes.
- **User-visible features ship with acceptance cases.** Add rows to
  [`docs/compat-checklist.md`](docs/compat-checklist.md) and one case file per row under
  `tests/gui/cases/` (format: [`tests/gui/README.md`](tests/gui/README.md)). Anything a case must
  assert or click goes into DebugState (`crates/gilvt-app/src/debug_state/`, documented in
  [`docs/debug-state.md`](docs/debug-state.md)). `tests/gui/selftest.sh` checks that rows and cases
  match.
- GUI acceptance runs locally only (it needs a desktop session, Peekaboo and screen permissions), so it
  is not part of CI. Say in the pull request which cases you ran.
- New agent behavior: update the real-format fixtures first, then the fake agent scenarios in
  `crates/gilvt-fake-agent/scenarios/`.
- When user-visible behavior changes, update both user guides together: `docs/user-guide.md` (English),
  `docs/user-guide.zh-CN.md` (Chinese) and their `.html` editions.

### Commits and pull requests

- Commit messages follow `type(scope): summary`, for example
  `fix(gilvt-app): settings window colors follow the theme`. Types: `feat`, `fix`, `docs`, `test`,
  `refactor`, `chore`.
- One topic per pull request. Fill in the template, including AI disclosure and the tests you ran.
- By contributing, you agree that your contribution is licensed under the
  [Apache License 2.0](LICENSE), the project's license.
