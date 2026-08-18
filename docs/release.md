# Worker release

`harness-e2e` owns its release pipeline in this independent repository. The
pipeline materializes a version in the public Workers Registry, but deliberately
does not move `latest`: every repository-owned release publishes only `next`.
Promotion beyond `next` remains a separate, explicitly authorized operation.

## Release contract

A release starts only from a pushed tag matching `harness-e2e/vX.Y.Z`. The tag
version must equal `package.version` in `Cargo.toml`, and the tag, checked-out
commit, static `iii.worker.yaml`, and `config.yaml` must agree before any release
surface is mutated. Prerelease SemVer tags are rejected.

The workflow then:

1. builds `dashboard/dist` and the injectable `dashboard/dist-console` once and
   fingerprints every embedded byte as one release unit;
2. restores that exact bundle into all cross-compilation jobs;
3. attaches checksummed archives for the standard nine targets to a GitHub
   prerelease;
4. starts an isolated iii engine and the released
   `x86_64-unknown-linux-gnu` binary;
5. collects every function schema with `engine::functions::info` and fails if a
   request or response is empty/`AnyValue`;
6. constructs the Registry binary map from the uploaded checksum assets; and
7. POSTs the immutable version with `tag: next`.

The supported artifact targets are:

- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`
- `i686-pc-windows-msvc`
- `aarch64-pc-windows-msvc`
- `x86_64-unknown-linux-gnu`
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`
- `armv7-unknown-linux-gnueabihf`

Unix assets use `harness-e2e-<target>.tar.gz`; Windows assets use
`harness-e2e-<target>.zip`. Every target also has a
`harness-e2e-<target>.sha256` asset.

## Protected publication

Configure a GitHub environment named `workers-registry-next` with required
reviewers and store `WORKERS_REGISTRY_API_KEY` as an environment secret. The
secret is scoped only to the final Registry job and only to the POST step; the
validation, frontend, binary, and interface-collection steps cannot read it.

The iii CLI version used for interface collection is pinned to the same protocol
line as `iii-sdk` in `.github/workflows/release.yml`. The interface project also
installs the manifest's exact `state@0.22.0` dependency and waits for
`state::list` before starting `harness-e2e`. Update either pin as an intentional
release-pipeline change, never dynamically during a release.

## Cutting a release

1. Update `version` in `Cargo.toml` and refresh `Cargo.lock`.
2. Land the release commit on the intended branch and require normal CI to pass.
3. Create a signed or annotated tag that points at that exact commit:

   ```bash
   git tag -s harness-e2e/vX.Y.Z -m 'harness-e2e vX.Y.Z'
   git push origin harness-e2e/vX.Y.Z
   ```

4. Verify the GitHub prerelease contains all nine archives and checksums, then
   verify `harness-e2e@next` resolves to `X.Y.Z` in the Registry.

Do not move or recreate a published tag. If a build fails before Registry
publication, fix the cause, bump to the next version, and create a new tag.
An HTTP 409 is accepted only when the immutable version already exists and
`next` still resolves to the exact same version; every other conflict fails
closed.

The interface collection configuration is intentionally side-effect-minimal in
`config.collect.yaml`. It proves registration and schema quality; it does not
execute a model-backed E2E assessment or replace the normal release validation
suite.
