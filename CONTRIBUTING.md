# Contributing

This repository is a personal fork of upstream yabai. Upstream does not accept
pull requests, so changes here should stay small, focused, and easy to rebase.

## Commit Messages

All commits must use [Conventional Commits](https://www.conventionalcommits.org/):

```text
<type>(<scope>): <summary>
<type>: <summary>
```

Use concise imperative summaries, for example:

```text
fix(window-manager): recompute invalid view frame
docs: document release workflow
build: add local dev target
```

Preferred types are `fix`, `feat`, `docs`, `build`, `ci`, `refactor`, `test`, and
`chore`. Use `!` after the type or scope, or add a `BREAKING CHANGE:` footer, for
breaking changes.

## Patch Guidelines

- Keep each patch focused and well described.
- Match the surrounding upstream C style.
- Avoid broad formatting-only changes.
- Update `CHANGELOG.md` and version metadata when preparing a release.
