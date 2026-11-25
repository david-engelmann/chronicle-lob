# Security policy

## Supported versions

Security fixes apply to the newest published release of each
crate and to the main branch. The crates are chronicle-core
and chronicle-wal, with the chronicle-lob-cli binary. Older
releases are not patched; upgrading to the newest release is
the fix path.

## Reporting a vulnerability

Please report suspected vulnerabilities privately through
GitHub's private vulnerability reporting for this repository,
the Report a vulnerability button under the Security tab.
Please do not open a public issue for a suspected
vulnerability. Include the affected crate and version, a
description of the flaw, and the smallest reproduction you
can share. Reports are read by the maintainer, and a fix or
a reasoned answer follows from there.

## Scope

The write-ahead log and snapshot decoders read untrusted
bytes by design. They are built to reject malformed input
with typed errors and never to panic, and a seeded mutation
harness exercises them at up to one hundred thousand cases
per surface (see INV-WAL-7 in docs/INVARIANTS.md). A panic
or an accepted malformed record in either decoder counts as
a security issue under this policy. The matching engine is
deterministic and single threaded, and a pure performance
problem is an ordinary bug rather than a vulnerability,
unless untrusted input can use it to halt a host process.
