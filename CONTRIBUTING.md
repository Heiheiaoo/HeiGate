# Contributing to HeiGate

Thank you for helping improve HeiGate. Contributions should be focused,
reviewable, tested, and compatible with the project's LGPL-3.0-or-later
license.

## Before You Start

- Search existing issues and pull requests before opening a duplicate.
- Use an issue for substantial behavior changes, security-sensitive designs,
  database migrations, or public API changes before implementation.
- Do not open a public issue for a suspected vulnerability. Follow
  [SECURITY.md](SECURITY.md).
- Do not include credentials, customer data, proprietary code, generated
  build output, or third-party material without redistribution rights.

## Development Setup

Required tools:

- Go version declared in `backend/go.mod`
- Node.js 20 or later
- pnpm 9 or later

Install frontend dependencies:

```bash
pnpm --dir frontend install --frozen-lockfile
```

Run the standard checks:

```bash
go test ./...
pnpm --dir frontend run lint:check
pnpm --dir frontend run typecheck
pnpm --dir frontend run test:run
pnpm --dir frontend run build
```

Run Go commands from `backend/` unless the command explicitly uses a path.

## Pull Requests

- Keep each pull request limited to one coherent change.
- Add or update tests for behavior changes.
- Document new configuration, migrations, APIs, and operational risks.
- Preserve backwards compatibility unless the issue and pull request clearly
  describe the migration path.
- Update `NOTICE` when adding material that requires attribution.
- Complete the pull request template and disclose use of generated code when
  it is material to the change.

## Commit Sign-Off

HeiGate uses the Developer Certificate of Origin 1.1 instead of a separate
Contributor License Agreement. Sign off every commit with:

```bash
git commit -s -m "type: concise description"
```

The sign-off certifies that you have the right to submit the contribution under
the project's license. Read the full DCO at:
https://developercertificate.org/

## Commit Style

Use concise Conventional Commit-style subjects where practical:

- `feat: add latency-aware desktop routing`
- `fix: preserve API key when editing a channel`
- `docs: clarify Docker secret setup`
- `test: cover desktop failover behavior`

## Review Standards

Maintainers may request changes for correctness, security, maintainability,
test coverage, licensing, documentation, or scope. A contribution may be
closed when it cannot be maintained safely or does not fit the project.
