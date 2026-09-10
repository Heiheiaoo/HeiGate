# HeiGate Open Source Guide

This document describes the project's maintenance and release expectations.

## Project Status

HeiGate is independently maintained and derived from `Wei-Shaw/sub2api`.
Upstream changes may be incorporated selectively after review. Compatibility
with every upstream release is not guaranteed.

## Licensing

The repository is distributed under LGPL-3.0-or-later. Contributions are
accepted under the same terms and require a Developer Certificate of Origin
sign-off. Contributors must preserve applicable copyright and attribution
notices.

## Branches And Releases

- The default branch contains current development work.
- Releases use semantic version tags where practical.
- Breaking changes must be called out in release notes with a migration path.
- Database and configuration migrations should be backwards-aware and tested.
- Security releases may contain limited details until a fix is available.
- GitHub Releases publish the signed-off source tag as a macOS Apple silicon
  desktop package (`.dmg` and `.zip`); Docker images are not published.

## Compatibility Policy

Public HTTP APIs, configuration fields, environment variables, database data,
and plugin contracts are compatibility surfaces. Changes to these surfaces
should be additive when possible. Deprecations should be documented before
removal.

The supported distribution is the macOS desktop application. Desktop mode uses
local SQLite storage and must be tested on the supported macOS architecture.

## Maintenance Decisions

Maintainers prioritize security, correctness, data integrity, operability, and
long-term maintenance cost. A feature can be declined even when technically
valid if it creates unsafe defaults, unclear ownership, excessive complexity,
or an unsustainable support burden.

## Third-Party Services

HeiGate integrates with external model and payment providers. Names and marks
belong to their respective owners. Integrations do not imply endorsement, and
operators remain responsible for provider terms, local law, data protection,
and account security.
