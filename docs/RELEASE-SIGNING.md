# Release signing & the custom-build architecture

This document is the design + runbook for lait's signed release pipeline. It is
also intended as a **reusable template** for other Nixie projects (e.g. warren).

## Why lait doesn't use cargo-dist's built-in signing

cargo-dist (v0.32.0, the current release) cannot produce the artifacts we need:

- **macOS notarization is impossible with the built-in signer.** Its `macos-sign`
  runs a plain `codesign` with **no hardened runtime** (issue #1534, open since
  2024). A signature without hardened runtime *fails* Apple notarization, so its
  output can never be notarized or stapled.
- **Windows signing is SSL.com-only.** cargo-dist has no backend for **Azure
  Artifact Signing** (issue #1122/#2395), which is the CA we chose (~$120/yr vs
  ~$1,200/yr for SSL.com eSigner).
- **There is no post-build-pre-archive hook.** `github-build-setup` runs *before*
  `dist build`, and `dist build` compiles + archives atomically. There is nowhere
  to inject "sign the binary after compile, before it is tar/zipped."

To sign the **distributed** archives (the exact `.tar.gz`/`.zip` that
brew/scoop/winget/`curl|sh` hand out), the binary must be signed before
archiving. That requires owning the build.

## The architecture (the uv/ruff pattern)

Set in `dist-workspace.toml`:

```toml
build-local-artifacts = false
local-artifacts-jobs  = ["./build-binaries"]
```

cargo-dist then **replaces** its default per-target build job with a call to our
reusable workflow `.github/workflows/build-binaries.yml` (`on: workflow_call`),
passing the release `plan` as a string input. cargo-dist still owns everything
else: the `plan`, the installers, the unified `sha256.sum`, GitHub Release
creation, and the publisher fan-out.

### The contract build-binaries.yml MUST satisfy

Per target, build the binary, **sign it**, then hand-roll the archive in the
exact shape cargo-dist's plan advertises, and upload it under an `artifacts-*`
name. cargo-dist's `host` job collects everything matching `artifacts-*` by
**filename** — so the filenames and in-archive layout are the contract:

| Requirement | Value |
|---|---|
| Unix archive name | `lait-<target-triple>.tar.gz` (+ `.tar.gz.sha256`) |
| Windows archive name | `lait-<target-triple>.zip` (+ `.zip.sha256`) |
| In-archive layout (unix) | `lait-<target>/` containing `lait`, `CHANGELOG.md`, `LICENSE-APACHE`, `LICENSE-MIT`, `README.md` |
| In-archive layout (windows) | flat: `lait.exe` + the 4 misc files at the zip root |
| Upload artifact name | `artifacts-<target>` (matches the host job's `artifacts-*` glob) |

The nested-on-unix / flat-on-windows split is the same layout the self-updater
(`update_bin_path_in_archive`) and the `binstall` metadata encode. Keep all three
in lockstep.

The misc files matter: lait does **not** set `auto-includes = false`, so the plan
lists `CHANGELOG.md`, `LICENSE-APACHE`, `LICENSE-MIT`, `README.md` inside each
archive. The hand-rolled archive must include them to match.

### Signing insertion points

- **macOS** (`macos-14`/`macos-15` runner, per target):
  1. `cargo build --release --locked --target <t>`
  2. `codesign --force --options runtime --timestamp -s "$DEVELOPER_ID" target/<t>/release/lait`
  3. Build the archive (binary + misc, nested).
  4. Build a `.pkg` wrapping the signed binary (`pkgbuild`), notarize it
     (`xcrun notarytool submit --wait`), and `xcrun stapler staple` it — a bare
     Mach-O can't be stapled, the `.pkg` can. Upload the stapled `.pkg` as an
     **additional** asset `lait-<target>.pkg` for the offline-clean GUI path; the
     tarball binary shares the notarized cdhash so Gatekeeper's online lookup also
     clears it.
- **Windows** (`windows-latest`, x86_64):
  1. `cargo build --release --locked --target x86_64-pc-windows-msvc`
  2. Azure-sign the `.exe` **before** zipping, via
     `azure/artifact-signing-action` (OIDC to Azure; no stored cert).
  3. `7z a lait-<target>.zip lait.exe <misc>` + `sha256sum`.
- **Linux** (no OS signing): build + archive. Provenance attestation covers it.

### Attestation

Because the default job's `actions/attest` step is gone, add an attestation step
to each build-binaries job (after archiving, `subject-path` = the archive), or a
single global attest over all archives. Requires `id-token: write` +
`attestations: write` job permissions (set via `github-custom-job-permissions`).

## Secret-gating (so main stays releasable before accounts exist)

Every signing step is gated on its secret being present and **soft-skips** when
absent — the same pattern as `publish-homebrew/scoop/winget`. A release cut
before the certs land produces **unsigned** archives (still valid, still
installable); the first release after the secrets are added is signed. The custom
job also runs on `pull_request` (build-only, no signing) so the build path is
CI-tested without a tag.

### Required repository secrets

| Secret | For | Source |
|---|---|---|
| `APPLE_DEVELOPER_ID` | codesign identity ("Developer ID Application: Nixie Solutions LLC (TEAMID)") | Apple Developer (org enrollment) |
| `APPLE_CERT_P12` | base64 of the Developer ID cert + key `.p12` | Apple Developer |
| `APPLE_CERT_PASSWORD` | password for the `.p12` | you |
| `APPLE_NOTARY_KEY` / `APPLE_NOTARY_KEY_ID` / `APPLE_NOTARY_ISSUER` | App Store Connect API key for `notarytool` | App Store Connect |
| `AZURE_TENANT_ID` / `AZURE_CLIENT_ID` / `AZURE_SUBSCRIPTION_ID` | OIDC login for Azure Artifact Signing | Azure |
| `AZURE_SIGNING_ACCOUNT` / `AZURE_SIGNING_PROFILE` | the Artifact Signing account + cert profile | Azure |

All certs must be issued to the legal entity **Nixie Solutions LLC** (distinct
from the `Nixie-Tech-LLC` GitHub org slug). Apple org enrollment needs a D-U-N-S
number for that exact name — request it early; it is the long pole.

## Verification strategy (given it can't run locally)

1. `dist plan` / `dist generate` must stay consistent (CI's `release-dry-run`
   enforces this on every PR).
2. The `pull_request` trigger builds every target **without** signing, proving
   the build + hand-rolled archive path on real runners.
3. First **real** signed release is a `dev`-channel or `-rc` tag, verified with
   `gh attestation verify`, `codesign -dv --verbose=4`, `spctl -a -vvv -t install`
   (pkg), and `Get-AuthenticodeSignature` (Windows) before a stable tag.

## Migration steps (incremental, each independently valid)

1. Flip `build-local-artifacts = false` + `local-artifacts-jobs`, add a
   build-binaries.yml that reproduces **today's unsigned** archives exactly
   (Linux/macOS/Windows), regenerate `release.yml`, confirm `dist plan` matches.
   → releases keep working, unsigned, on the new architecture.
2. Add macOS codesign + notarize + `.pkg`, secret-gated.
3. Add Windows Azure signing, secret-gated.
4. Add attestation into the custom job.
5. Flip the first signed `-rc` release; verify; then stable.
