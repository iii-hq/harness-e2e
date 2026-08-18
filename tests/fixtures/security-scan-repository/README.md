# Security scan E2E fixture template

This directory is intentionally vulnerable test data for the private
`iii-hq/security-scan-e2e-fixture` repository. Every credential-shaped value is
fake. Do not replace it with a real credential and do not use this directory as
an application dependency.

Expected seeded areas:

- `src/vulnerable.rs`: command injection;
- `package.json`: old and unpinned dependencies;
- `.env.example`: an explicitly fake token-shaped value;
- `.github/workflows/insecure.yml`: disabled, intentionally unsafe supply-chain
  patterns.

The private fixture repository must separately enable and seed at least one
Dependabot alert and one code-scanning alert. The E2E intentionally validates
coherent, non-null collected counts rather than exact counts.
