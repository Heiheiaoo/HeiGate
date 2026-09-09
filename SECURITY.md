# Security Policy

## Supported Versions

Security fixes are provided for the latest released version and the current
default branch. Older releases may not receive backports.

## Reporting a Vulnerability

Do not disclose suspected vulnerabilities in public issues, discussions, pull
requests, or chat rooms.

Use GitHub's private vulnerability reporting feature from the repository's
Security tab. Include:

- affected version or commit;
- affected endpoint, component, or deployment mode;
- reproduction steps or a minimal proof of concept;
- impact and realistic attack conditions;
- suggested mitigation, if known.

Remove API keys, access tokens, personal data, and production records from all
reports. Use synthetic examples wherever possible.

Maintainers will acknowledge actionable reports when capacity permits, assess
severity, coordinate a fix, and publish an advisory after users have had a
reasonable opportunity to update. Please allow time for remediation before
public disclosure.

## Security Boundaries

- Desktop mode is intended to bind to loopback and store data for one local
  operating-system user. Do not expose its management API to a network.
- Server mode must be deployed behind properly configured TLS and trusted
  reverse proxies.
- Operators are responsible for rotating secrets, restricting data-directory
  permissions, backing up databases, and complying with upstream provider
  terms.
- The project does not guarantee that using subscription credentials through
  a gateway is permitted by every upstream provider.
