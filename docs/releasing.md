# Releasing Dromon

A release is one run of `.github/workflows/release.yml`: you pick patch, minor or major and
approve once; it bumps the version, builds, packages and signs every file, tags the commit with
the plain version (`0.1.0`, no `v`), publishes the GitHub release, and brings the version commit
to `main`.

| File | For | Signed with |
|---|---|---|
| `Dromon-<v>-windows-x64.msi` | Windows 11 x64 (Windows on Arm runs it emulated) | Authenticode (SignPath Foundation, SSL.com eSigner or Azure Artifact Signing), else unsigned |
| `Dromon-<v>-macos-arm64.dmg` | macOS 14+, Apple silicon (not Intel Macs) | Developer ID + notarization, else ad-hoc |
| `Dromon-<v>-linux-x64.deb` (+ `.deb.asc`) | Debian 12+, Ubuntu 22.04+, x86_64 | release GPG key |
| `Dromon-<v>-linux-x64.pkg.tar.zst` (+ `.pkg.tar.zst.sig`) | Arch Linux, x86_64 | release GPG key |
| `SHA256SUMS` (+ `SHA256SUMS.asc`), `dromon-release-key.asc` | every file | release GPG key |
| (every file) | | Sigstore build-provenance attestation |

No signing credentials exist yet, and none is needed to start: without them every job still
runs and the files ship unsigned (Windows), ad-hoc signed (macOS) or without GPG signatures
(Linux, check runs only). [Signing](#signing) says what to set up, free options first.

## Cutting a release

1. **Run it**: Actions → Release → *Run workflow*, branch `main`, and pick **patch**, **minor**
   or **major** (from `0.1.0`: `0.1.1`, `0.2.0`, `1.0.0`). Or:
   ```sh
   gh workflow run release.yml -R fatihdogmus/dromon --ref main -f bump=minor
   ```
2. **Approve once.** `plan` names the version in its summary and commits the bump (`Cargo.toml`,
   `Cargo.lock`) to the branch `release/<version>`; the `approve` job then waits for you
   (Review deployments → `release-approval`). The builds run meanwhile; nothing is signed before
   you approve. With `WINDOWS_SIGNING=signpath`, SignPath also asks for its own approval.
3. **That's it** (roughly 40-60 minutes). At the end `publish` attests every file, tags the
   version commit `<version>`, and publishes the release; its notes list the commits since the
   previous release, then every file with its SHA-256 and how it is signed. `land` brings the
   version commit to `main` (a fast-forward, or a merge commit if `main` moved during the run)
   and deletes `release/<version>`; `homebrew` updates the cask in the tap.

Nothing is tagged, published or pushed to `main` before the very end, so a run that fails or is
cancelled leaves only the `release/<version>` branch, which the next run overwrites. Re-run the
failed jobs from the Actions page, or start a new run. A published release and its tag are never
changed: to fix one, release the next version.

**Dry runs.** The same, with *Dry run* ticked (from any branch): the same builds and packages at
the current version, signed with whatever credentials exist (after your approval), and nothing
bumped, tagged, published or pushed: the files stay as the run's artifacts (`release-files`,
`sign-linux`, `release-notes` and the per-platform builds). Use it after changing packaging or
signing setup.

## What the workflow does

It compiles, packages, signs and publishes; it runs no lint and no tests (the commit on `main`
was tested by `ci.yml`). The only checks are part of packaging and signing: each build script
runs the packaged `dromon --version`, and each signing step verifies the signature it just made.

```
plan ─┬─ approve (release-approval: you) ── credentials (release env) ──────────────┐
      ├─ build-linux (x64, ubuntu-22.04) ── package-arch ───────────────────────────┤
      ├─ build-macos (macos-15, arm64) ── sign-macos (release env)* ────────────────┤
      └─ build-windows (x64) ── sign-windows-exe (release env)*                     │
                                    ── package-windows (x64)                        │
                                    ── sign-windows-msi (release env)* ─────────────┤
         assemble ◄─────────────────────────────────────────────────────────────────┘
            └── sign-linux (release env)* ── publish (release env) ─┬─ land
                                                                    └─ homebrew (homebrew env)
         * runs only when its credentials exist; publish, land and homebrew: releases only
```

To check packages by hand the way a user would (install, run, uninstall), the scripts are still
there: `scripts/package-linux.sh --verify` (Debian, Ubuntu, Arch containers),
`packaging/macos/verify-dmg.sh` and `packaging/windows/verify-msi.ps1`.

| Job | Needs | Environment | Produces |
|---|---|---|---|
| `plan` | | — | mode (`release`/`check`), version, tag; for a release the version commit on `release/<version>` (`set-version.sh`), whose SHA every job builds |
| `approve` | plan | `release-approval` | nothing: the run's one approval |
| `credentials` | plan, approve | `release` | which credentials exist (`windows`, `macos`, `gpg`); enforces the [policy](#credentials-policy) |
| `build-linux` ×2 | plan | — | `.deb` (binary built on Ubuntu 22.04: glibc 2.35) |
| `package-arch` | build-linux | — | both `.pkg.tar.zst`, made by makepkg from the `.deb`s |
| `build-macos` | plan | — | `Dromon.app.zip` (Apple silicon, ad-hoc signed) and the unsigned DMG |
| `build-windows` ×2 | plan | — | `dromon.exe` with its version resource and icon |
| `sign-windows-exe` | credentials, build-windows | `release` | Authenticode-signed `dromon.exe` (azure, sslcom) |
| `package-windows` ×2 | build-windows, sign-windows-exe | — | the MSI around the signed (or unsigned) exe |
| `sign-windows-msi` | credentials, package-windows | `release` | signed MSIs (azure, sslcom; signpath signs exe-in-MSI and MSI here) |
| `sign-macos` | credentials, build-macos | `release` | signed, notarized, stapled app in a signed, notarized, stapled DMG |
| `assemble` | every build and signing job | — | `release-files` (the 7 files + `SHA256SUMS`), `release-notes` |
| `sign-linux` | assemble | `release` | `.deb.asc`, `.pkg.tar.zst.sig`, `SHA256SUMS.asc`, the public key |
| `publish` | assemble, sign-linux | `release` | attestations; the tag and the published release (releases only) |
| `land` | publish | — | `main` gets the version commit; `release/<version>` is deleted |
| `homebrew` | publish | `homebrew` | the cask in the tap ([Homebrew](#homebrew)) |

### The security boundary

`.github/workflows/release.yml` and `.github/release/` are the boundary. Review changes to them
as carefully as code that holds the keys (a CODEOWNERS rule for both is worth adding).

- **Build and package jobs** run repository code (cargo, the packaging scripts, the app
  itself) with no environment, no secrets and no `id-token`. They produce unsigned files.
- **`credentials`, `sign-*` and `publish`** run in the `release` environment, which holds every
  signing secret and variable and admits only `main`, and only after `approve`, the one job you
  approve (environment `release-approval`). They check out only `/.github/release/` (sign-macos also reads
  `packaging/macos/Info.plist`, as data), run only those scripts and SHA-pinned actions, and
  never build, install project dependencies, or run anything from the artifacts.
- **Artifacts are data with a digest.** Every artifact carries an `ARTIFACT-SHA256SUMS`; the job
  that made it reports that manifest's SHA-256 as a job output, and every job that downloads it
  checks the digest, every file, and that nothing was added (`.github/release/upload`,
  `.github/release/download`). An artifact replaced after its job finished is rejected.
- **Signing jobs only add signatures.** `sign-linux` writes detached signatures beside files it
  never changes; the Windows and macOS jobs sign in place and verify again.
- **Secrets stay out of logs**: the scripts run with `set +x`; GitHub masks secret values; the
  GPG passphrase reaches gpg on a file descriptor; the credentials job sees only whether a secret
  is set (`secrets.X != ''`), never its value; the keychain and `GNUPGHOME` are deleted afterwards.
- **Provenance**: `publish` attests every file with `actions/attest-build-provenance` (Sigstore,
  keyless): anyone can check that a file came from this workflow, run from `main`.

### Credentials policy

| Platform | Credentials missing | Partly set |
|---|---|---|
| Linux (GPG) | dry run: warning, no GPG signatures. **Release: fails** (the key is free). | fails |
| Windows | the files ship **unsigned**, with a warning in the run summary and the release notes. A release fails when the repository variable `DROMON_REQUIRE_SIGNED_WINDOWS` is `true`. | fails (`WINDOWS_SIGNING` set, its values incomplete) |
| macOS | the app ships **ad-hoc signed, not notarized**, with a warning. A release fails when `DROMON_REQUIRE_SIGNED_MACOS` is `true`. | fails |

Set the `DROMON_REQUIRE_SIGNED_*` variables once a platform's signing works, so a lost secret can
never silently ship an unsigned release.

## Signing

### Free options

Start here; each is independent of the others.

- **Linux: GPG (free).** A dedicated release key signs the `.deb`s, the Arch packages and
  `SHA256SUMS`. Required for releases: [set it up](#2-linux-the-release-gpg-key) first.
- **Every file: Sigstore build-provenance attestations (free).** Already on, nothing to set up
  for a public repository. Users check any file with `gh attestation verify`. (A private
  repository needs GitHub Enterprise Cloud for attestations.)
- **Windows: SignPath Foundation (free for open source).** SignPath Foundation signs with a
  certificate issued to *SignPath Foundation*, so that is the publisher users see. Its
  [conditions](https://github.com/SignPath/fdn-website/blob/main/docs/terms.md), in short:
  an OSI license and no proprietary components (MIT: yes); actively maintained, **already
  released** in the form to be signed (so the first public releases ship unsigned), and
  described on its download page; you sign only your own code, built by CI from this repository;
  **MFA** on GitHub and SignPath for every team member; **every release approved by hand** in
  SignPath; product name and version metadata enforced on every signed binary; and a
  **"Code signing policy"** section on the project's page ([draft below](#code-signing-policy-for-the-readme)).
  SignPath decides whom it accepts. [Set it up](#3a-windows-signpath-foundation-free).
- **Windows fallback: unsigned.** The default. Users see SmartScreen's "Windows protected your
  PC" and click *More info → Run anyway*; the release notes say so, and how to check the file.
- **macOS: there is no free trusted path.** Developer ID signing and notarization require the
  Apple Developer Program. **Fallback: ad-hoc signed** (the default). Apple silicon runs nothing
  unsigned, so the app is ad-hoc signed; Gatekeeper blocks the first open of a downloaded copy,
  and users allow it in *System Settings → Privacy & Security → Open Anyway* (macOS 15 and later
  have no right-click → Open bypass) or with `xattr -dr com.apple.quarantine /Applications/Dromon.app`.
  The release notes include both.

### Paid options

- **Windows, individuals outside the US and Canada: SSL.com eSigner OV code signing.** An OV
  certificate validated for you as an individual, kept in SSL.com's cloud HSM and used through
  CodeSignTool (no USB token). The publisher shown is your validated legal name.
  [Set it up](#3b-windows-sslcom-esigner).
- **Windows: Azure Artifact Signing** (about US$10/month). Microsoft's
  [quickstart](https://learn.microsoft.com/azure/artifact-signing/quickstart) allows Public Trust
  certificates for **individual developers only in the US and Canada**, and for
  **organizations** in the US, Canada, the EU and the UK. (The quickstart now also lists
  Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel for
  organizations; some older Microsoft pages still name only the first four.) It needs a paid
  Azure subscription. [Set it up](#3c-windows-azure-artifact-signing).
- **macOS: Apple Developer Program** (US$99/year): a Developer ID Application certificate and
  notarization. [Set it up](#4-macos-developer-id-and-notarization).

None of the Windows options gives instant SmartScreen reputation: a new publisher's downloads
show a warning until they build up reputation over weeks of installs. Keep the same signing
identity from release to release.

### 1. The `release` environment

Every signing secret and variable lives in the GitHub environment `release` (Settings →
Environments), which admits only `main`. Your one approval per run is a second environment,
`release-approval`, with you as its required reviewer and no secrets: GitHub asks again for
every job that enters a reviewed environment once it becomes ready, so the secrets' environment
has no reviewer and its jobs wait on `approve` instead.

```sh
REPO=fatihdogmus/dromon
ME=$(gh api user --jq .id)
# release: the secrets; main only, no reviewer.
gh api -X PUT "repos/$REPO/environments/release" --input - <<'EOF'
{ "reviewers": [], "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true } }
EOF
gh api -X POST "repos/$REPO/environments/release/deployment-branch-policies" -f name=main -f type=branch
# release-approval: you approve each run here (any branch, for dry runs).
gh api -X PUT "repos/$REPO/environments/release-approval" --input - <<EOF
{ "reviewers": [{ "type": "User", "id": $ME }], "prevent_self_review": false, "deployment_branch_policy": null }
EOF
```

A dry run from another branch builds and packages, then stops at `credentials`: `release`
admits only `main`. Run dry runs that should sign from `main`.

**Tag ruleset**: a tag, once made, can't be moved or deleted (admins excepted). Creating one
stays open to anyone with write access, so the workflow's token can tag releases: a personal
repository can't name GitHub Actions as a ruleset bypass actor (organizations can, as
`{ "actor_type": "Integration", "actor_id": 15368 }`, and could then restrict creation too).

```sh
gh api -X POST "repos/$REPO/rulesets" --input - <<'EOF'
{
  "name": "Release tags",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/tags/*"], "exclude": [] } },
  "rules": [{ "type": "update" }, { "type": "deletion" }, { "type": "non_fast_forward" }],
  "bypass_actors": [{ "actor_type": "RepositoryRole", "actor_id": 5, "bypass_mode": "always" }]
}
EOF
```

`actor_id` 5 is the repository *Admin* role. `land` pushes the version commit to `main` with
the workflow's token: protecting `main` (pull requests, required CI) would block it unless the
token may bypass. Keep 2FA on your GitHub account.

Setting values (secrets from stdin or a file, never from your shell history):

```sh
gh secret set NAME --env release < file          # or: printf %s "$VALUE" | gh secret set NAME --env release
gh variable set NAME --env release --body value  # variables are not secret
gh variable set DROMON_REQUIRE_SIGNED_WINDOWS --body true   # a repository variable (no --env)
```

### 2. Linux: the release GPG key

**Done.** The key exists and is configured:

- `Dromon Releases <fdogmus06@gmail.com>`, Ed25519, signing only, expires 2028-11-24.
  Fingerprint `61AA 2003 0B81 70BE 3527  E4BA 1E54 8E51 E3AD 8C38`.
- The `release` environment holds `RELEASE_GPG_PRIVATE_KEY`, `RELEASE_GPG_PASSPHRASE` and
  `RELEASE_GPG_FINGERPRINT`.
- The public key is committed as `KEYS` (with verify instructions) and published on
  keys.openpgp.org; `validpgpkeys` in `packaging/arch/PKGBUILD.aur` has the fingerprint.
- The offline backup (private key, passphrase, revocation certificate) belongs in the
  maintainer's password manager, not on a build machine.

`KEYS` must hold the key whose fingerprint is `RELEASE_GPG_FINGERPRINT`
(`scripts/package-linux.sh --verify` and users check against it); every release also carries the
key as `dromon-release-key.asc`.
`sign-linux.sh` refuses a secret key whose fingerprint isn't `RELEASE_GPG_FINGERPRINT`, or an
export that holds more than one key.

Extending it before it expires (with the offline keyring as `GNUPGHOME`):

```sh
gpg --quick-set-expire 61AA20030B8170BE3527E4BA1E548E51E3AD8C38 2y
gpg --armor --export-secret-keys 61AA20030B8170BE3527E4BA1E548E51E3AD8C38 | gh secret set RELEASE_GPG_PRIVATE_KEY --env release
gpg --armor --export 61AA20030B8170BE3527E4BA1E548E51E3AD8C38   # paste into KEYS, commit
```

Replacing it with a new key: generate one the same way (`gpg --quick-generate-key '<name> <email>'
ed25519 sign 2y` in a fresh `GNUPGHOME`), set the two secrets and the variable, replace the key
block in `KEYS`, and update `validpgpkeys`.

### 3a. Windows: SignPath Foundation (free)

1. Make sure the conditions above hold: a public release exists (an unsigned one counts), the
   download page describes Dromon, MFA is on, and the README has the code signing policy.
2. Apply at [signpath.org](https://signpath.org). SignPath Foundation reviews the project; once
   it accepts, it sets up an organization and a project for Dromon in
   [app.signpath.io](https://app.signpath.io), with its certificate.
3. In SignPath:
   - Project → *Trusted build systems*: link the predefined **GitHub.com** system. Optionally
     install the [SignPath GitHub App](https://github.com/apps/signpath).
   - Project → *Artifact configurations*: add one from
     [`packaging/signpath/dromon-msi.xml`](../packaging/signpath/dromon-msi.xml) and note its
     slug (or make it the default). It deep-signs each MSI, `dromon.exe` inside it and then the
     MSI, in **one request per release**. It enforces ProductName `Dromon` and ProductVersion =
     the release version on the exe (`packaging/windows/build-exe.ps1` stamps them), and Subject
     and Author `Dromon` on the MSI (`packaging/windows/dromon.wxs`).
   - Project → *Signing policies*: the release policy (slug e.g. `release-signing`), with you as
     **Approver**.
   - Users: add a CI user (or use yours) with the **Submitter** role on that policy, and create an
     **API token** for it.
4. Into the release environment:
   ```sh
   gh variable set WINDOWS_SIGNING --env release --body signpath
   gh variable set SIGNPATH_ORGANIZATION_ID --env release --body <organization id>
   gh variable set SIGNPATH_PROJECT_SLUG --env release --body dromon
   gh variable set SIGNPATH_SIGNING_POLICY_SLUG --env release --body release-signing
   gh variable set SIGNPATH_ARTIFACT_CONFIGURATION_SLUG --env release --body msi   # optional: else the default
   gh secret set SIGNPATH_API_TOKEN --env release                                 # paste, then Ctrl-D
   ```
5. Each release, `sign-windows-msi` submits the request and waits (up to about 5 h 40 min, the
   job's limit) until you **approve it in SignPath**. It then checks the signatures (the
   publisher must be *SignPath Foundation*) and the run goes on.

SignPath gets the MSIs as a workflow artifact of the same run (`signpath-windows-msi`), uploaded
from files whose digests the run checked, and it verifies for itself that the run built them on
GitHub-hosted runners.

### 3b. Windows: SSL.com eSigner

1. Buy an **OV code signing certificate** at [SSL.com](https://www.ssl.com) with **eSigner**
   cloud signing (individual validation: your legal name becomes the publisher; expect identity
   and phone checks). Choose an eSigner plan that allows automated signing.
2. In the SSL.com dashboard, enrol the certificate in eSigner and set up its one-time-password
   factor **for automation**: SSL.com shows the TOTP secret (the base32 seed behind the QR code);
   keep it. The certificate's **credential ID** is in the order's eSigner section (or use
   CodeSignTool's `get_credential_ids`).
3. Into the release environment:
   ```sh
   gh variable set WINDOWS_SIGNING --env release --body sslcom
   gh secret set ESIGNER_USERNAME --env release        # your SSL.com account name
   gh secret set ESIGNER_PASSWORD --env release
   gh secret set ESIGNER_TOTP_SECRET --env release     # the base32 seed
   gh secret set ESIGNER_CREDENTIAL_ID --env release
   ```

`.github/release/sign-windows-sslcom.ps1` downloads CodeSignTool v1.3.2 for Windows from
SSL.com's official GitHub releases and refuses it unless its SHA-256 is the pinned
`4afc32e8b7f79bbe1de7e4e7049aaad4e0f754357613b9bbec0e3052f06fd36b`. It signs `dromon.exe` (before
the MSI is built) and then the MSIs, with SHA-256 and an RFC 3161 timestamp from
`http://ts.ssl.com`. To update CodeSignTool, download the new zip, check it, and change the URL
and the hash together.

### 3c. Windows: Azure Artifact Signing

1. Get a **paid** Azure subscription (not free or trial) and register the `Microsoft.CodeSigning`
   resource provider.
2. Create an **Artifact Signing account** in a supported region; its endpoint is, for example,
   `https://weu.codesigning.azure.net/` (West Europe).
3. Complete **identity validation** (Public Trust, organization or individual; portal only),
   then create a **certificate profile** of type *Public Trust*.
4. Create an **app registration** (Microsoft Entra ID) for GitHub OIDC, with no client secret. Add
   a *federated credential* with issuer `https://token.actions.githubusercontent.com` and subject
   `repo:fatihdogmus/dromon:environment:release`. Give it the **Artifact Signing Certificate
   Profile Signer** role on the account (or the profile).
5. Into the release environment (all variables; there is no secret):
   ```sh
   gh variable set WINDOWS_SIGNING --env release --body azure
   gh variable set AZURE_CLIENT_ID --env release --body <app (client) id>
   gh variable set AZURE_TENANT_ID --env release --body <tenant id>
   gh variable set AZURE_SUBSCRIPTION_ID --env release --body <subscription id>
   gh variable set AZURE_ARTIFACT_SIGNING_ENDPOINT --env release --body https://weu.codesigning.azure.net/
   gh variable set AZURE_ARTIFACT_SIGNING_ACCOUNT --env release --body <account name>
   gh variable set AZURE_ARTIFACT_SIGNING_PROFILE --env release --body <certificate profile name>
   ```

Artifact Signing certificates last a few days. The RFC 3161 timestamp
(`http://timestamp.acs.microsoft.com`) keeps signatures valid after that, and
`verify-windows-signatures.ps1` requires one.

### 4. macOS: Developer ID and notarization

1. Enrol in the [Apple Developer Program](https://developer.apple.com/programs/) (US$99/year), as
   an individual (your name is the publisher) or an organization (needs a D-U-N-S number).
2. **Developer ID Application certificate** (only the Account Holder can create one). On a Mac,
   Keychain Access → Certificate Assistant → *Request a Certificate From a Certificate Authority*
   (save it to disk). On
   [developer.apple.com](https://developer.apple.com/account/resources/certificates/list) →
   Certificates → + → **Developer ID Application** → upload the request → download and open the
   `.cer`. In Keychain Access → *My Certificates*, right-click *Developer ID Application: <name>
   (<TEAM ID>)* → Export → `.p12`, with a strong password.
3. **Notary key**: [App Store Connect](https://appstoreconnect.apple.com) → Users and Access →
   Integrations → App Store Connect API → *Team Keys* → + (access: *Developer*). Download
   `AuthKey_<KEY ID>.p8` (offered only once) and note the **Key ID** and the **Issuer ID**.
4. Into the release environment:
   ```sh
   base64 -i DeveloperID.p12 | gh secret set APPLE_DEVELOPER_ID_P12 --env release
   gh secret set APPLE_DEVELOPER_ID_P12_PASSWORD --env release          # the export password
   gh variable set APPLE_TEAM_ID --env release --body <TEAM ID>          # Membership details
   gh secret set APPLE_NOTARY_KEY --env release < AuthKey_<KEY ID>.p8
   gh variable set APPLE_NOTARY_KEY_ID --env release --body <KEY ID>
   gh variable set APPLE_NOTARY_ISSUER_ID --env release --body <issuer id>
   ```
   Then delete the local `.p12` and `.p8` copies (keep a backup offline).

`sign-macos` imports the certificate into a temporary keychain (deleted in an `always()` step),
signs `Dromon.app` with the hardened runtime and a secure timestamp, notarizes it with
`xcrun notarytool submit --wait` and staples it, builds the DMG with an `/Applications` link,
then signs, notarizes and staples the DMG.

**No entitlements.** The hardened runtime blocks JIT and unsigned executable memory, `DYLD_*`
variables, libraries not signed by Apple or the team, debugger attachment, and access to the
camera, microphone, location, contacts, calendars, photos and Apple Events. Dromon needs none of
these: it draws with Metal, links only system libraries, and starts `/usr/bin/ssh` and its own
signed executable (as ssh's askpass helper), which the hardened runtime allows. Add an
entitlement only together with the feature that needs it.

### Secrets and variables

All are in the `release` environment unless marked *repository*.

| Name | Kind | Used by |
|---|---|---|
| `RELEASE_GPG_PRIVATE_KEY` | secret | credentials (is it set?), sign-linux |
| `RELEASE_GPG_PASSPHRASE` | secret | sign-linux |
| `RELEASE_GPG_FINGERPRINT` | variable | credentials, sign-linux; the notes through `credentials` |
| `WINDOWS_SIGNING` (`azure`, `sslcom`, `signpath`; unset = unsigned) | variable | credentials → sign-windows-exe, sign-windows-msi, the notes |
| `SIGNPATH_API_TOKEN` | secret | credentials (is it set?), sign-windows-msi |
| `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG` | variables | credentials, sign-windows-msi |
| `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` (optional) | variable | sign-windows-msi |
| `ESIGNER_USERNAME`, `ESIGNER_PASSWORD`, `ESIGNER_TOTP_SECRET`, `ESIGNER_CREDENTIAL_ID` | secrets | credentials (is it set?), sign-windows-exe, sign-windows-msi |
| `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID` | variables | credentials, sign-windows-exe, sign-windows-msi (azure/login, OIDC) |
| `AZURE_ARTIFACT_SIGNING_ENDPOINT`, `AZURE_ARTIFACT_SIGNING_ACCOUNT`, `AZURE_ARTIFACT_SIGNING_PROFILE` | variables | credentials, sign-windows-exe, sign-windows-msi |
| `APPLE_DEVELOPER_ID_P12`, `APPLE_NOTARY_KEY` | secrets | credentials (is it set?), sign-macos |
| `APPLE_DEVELOPER_ID_P12_PASSWORD` | secret | sign-macos |
| `APPLE_TEAM_ID`, `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID` | variables | credentials, sign-macos |
| `DROMON_REQUIRE_SIGNED_WINDOWS`, `DROMON_REQUIRE_SIGNED_MACOS` | *repository* variables | credentials |
| `HOMEBREW_TAP_DEPLOY_KEY` | secret, *`homebrew` environment* | release.yml's `homebrew` job (pushes the cask) |

## Homebrew

`brew install --cask fatihdogmus/tap/dromon` installs from the tap
[fatihdogmus/homebrew-tap](https://github.com/fatihdogmus/homebrew-tap). Its `Casks/dromon.rb`
is written by `release.yml`'s `homebrew` job after publishing (to redo it, re-run that job).
On a macOS runner it:

1. skips the release unless it is GitHub's latest one and not a prerelease;
2. downloads `Dromon-<v>-macos-arm64.dmg` and `SHA256SUMS`, and checks the DMG's SHA-256 against
   its line and against the Sigstore attestation `release.yml` made (run from `main`);
3. asks Gatekeeper (`spctl --assess`) whether `Dromon.app` is notarized;
4. writes the cask (`.github/release/homebrew-cask.sh`) and pushes it to the tap.

While the app is not notarized, the cask's `postflight` removes the quarantine flag from
`Dromon.app` (Homebrew no longer has `--no-quarantine`, and only third-party taps may do this),
and its caveats say so. A notarized release drops both, with no change here.

The `homebrew` environment (deployments from `main`, no reviewer: the release run was the
approval) holds `HOMEBREW_TAP_DEPLOY_KEY`, the private half of a deploy key with
write access to the tap only. To replace it:

```sh
ssh-keygen -t ed25519 -N "" -C "dromon release homebrew" -f tapkey
gh repo deploy-key add tapkey.pub --repo fatihdogmus/homebrew-tap --allow-write --title "dromon release homebrew"
gh secret set HOMEBREW_TAP_DEPLOY_KEY --env homebrew --repo fatihdogmus/dromon < tapkey
shred -u tapkey tapkey.pub   # then delete the old key in the tap's Settings → Deploy keys
```

Without the secret the job writes the cask to its summary page instead of pushing it.

## Code signing policy (for the README)

SignPath Foundation requires this section on the project's home page and download page. Paste it
into the README once SignPath signs releases, and adjust the team list as the team grows:

```markdown
## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

- Committers and reviewers: [Muhammed Fatih Doğmuş](https://github.com/fatihdogmus)
- Approvers: [Muhammed Fatih Doğmuş](https://github.com/fatihdogmus)

Windows installers are built from this repository's source by its
[release workflow](.github/workflows/release.yml) on GitHub-hosted runners, signed only after an
approver reviews each release, and carry build-provenance attestations
(`gh attestation verify <file> --repo fatihdogmus/dromon`). Linux packages are signed with the
project's release GPG key (fingerprint `<FINGERPRINT>`).

### Privacy policy

This program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it. Dromon connects only to the SSH
servers you choose, through your system's OpenSSH client.
```

## Checking a download

The release notes carry these steps for each file. In short:

| What | How |
|---|---|
| Any file | `gh attestation verify <file> --repo fatihdogmus/dromon` |
| Any file (GPG) | `gpg --import dromon-release-key.asc` (check its fingerprint against the README), then `gpg --verify SHA256SUMS.asc SHA256SUMS` and `sha256sum --check --ignore-missing SHA256SUMS` |
| `.deb` | `gpg --verify Dromon-<v>-linux-x64.deb.asc Dromon-<v>-linux-x64.deb` |
| `.pkg.tar.zst` | `pacman -U` checks the `.sig` beside the package against pacman's keyring; trust the key there once: `sudo pacman-key --add dromon-release-key.asc && sudo pacman-key --lsign-key 61AA20030B8170BE3527E4BA1E548E51E3AD8C38` |
| `.msi` | `Get-AuthenticodeSignature .\Dromon-<v>-windows-x64.msi` → `Valid`. Unsigned: SmartScreen *More info → Run anyway* |
| `.dmg` | `spctl -a -t open --context context:primary-signature -v <dmg>`, `codesign --verify --deep --strict -v /Applications/Dromon.app`, `spctl -a -t exec -vv /Applications/Dromon.app`. Not notarized: *Privacy & Security → Open Anyway*, or `xattr -dr com.apple.quarantine /Applications/Dromon.app` |

## Windows: no console window

Release builds are Windows GUI-subsystem programs
(`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` in
`crates/dromon-app/src/main.rs`), so starting Dromon opens no console window. Debug builds
(tests, `cargo run`) keep the console.

The askpass helper still answers ssh, because ssh reads the helper's **stdout pipe**, not a
console. Win32-OpenSSH's `ssh_askpass` (`readpass.c`, the `FORK_NOT_SUPPORTED` branch) creates a
pipe and starts the helper with `posix_spawnp` and
`posix_spawn_file_actions_adddup2(p[1], STDOUT_FILENO)`. Its `posix_spawn_internal`
(`contrib/win32/win32compat/w32fd.c`) passes that pipe as `STARTUPINFO.hStdOutput` with
`STARTF_USESTDHANDLES`. A GUI-subsystem process inherits standard handles passed this way like
any other process, and Rust's `io::stdout()` writes to `GetStdHandle(STD_OUTPUT_HANDLE)`, which is
that pipe. When there is no handle at all (Dromon started from Explorer), Rust discards stdout
writes instead of failing. As a side benefit, ssh starting a GUI-subsystem helper never flashes a
console window.

As a consequence, `dromon --version` typed in `cmd` or PowerShell prints nothing visible (a GUI
program doesn't attach to its parent's console), but captured output works:
`& "C:\Program Files\Dromon\dromon.exe" --version | Out-String` (PowerShell then also waits for
it). `verify-msi.ps1` and `build-exe.ps1` read it that way. Not yet verified on Windows: the
askpass round trip itself (Dromon's Windows askpass transport isn't implemented in `dromon-ssh`
yet), and that GitHub's runners behave as described.

## Building packages locally

The workflow calls these scripts, and they run on a developer machine too:

```sh
scripts/package-linux.sh --verify              # x64 .deb + .pkg.tar.zst in target/packages, then installs
                                               # both in debian:12, ubuntu:24.04 and archlinux:latest
scripts/package-linux.sh --arch arm64          # arm64 (Docker emulation: slow; native on arm64 hosts)
packaging/macos/build-app.sh /tmp/out          # macOS: Dromon.app, its zip, the unsigned DMG
packaging/windows/build-exe.ps1 -Arch x64 -Out out                       # Windows: dromon.exe
packaging/windows/build-msi.ps1 -Exe out\dromon.exe -Arch x64 -Out out   # needs WiX v5
packaging/windows/verify-msi.ps1 -Msi out\Dromon-<v>-windows-x64.msi -Version <v>   # elevated shell
python3 packaging/icon/generate.py             # icon PNGs, .ico and .icns from the shapes in it
```

`package-linux.sh` builds in an `ubuntu:22.04` container by default, so the binary needs only
glibc 2.35 whatever the host runs. It keeps its own target folder and Docker volumes for the
cargo and rustup caches. `--build host` builds on the host, as CI does on its ubuntu-22.04 runners.

## Known limits and first-run checks

The Linux half is verified end to end locally; the rest needs GitHub's runners. On the first
check run, look at:

- **Targets**: Windows x64, macOS Apple silicon and Linux x86_64 only, for now. Adding one means
  a build job for it in `release.yml` (the packaging scripts already take `--arch` / `-Arch`;
  `packaging/macos/build-app.sh` notes how to make a universal binary for Intel Macs), its file
  name in `assemble.sh` and a row in `release-notes.sh`.
- **SignPath**: `packaging/signpath/dromon-msi.xml` follows SignPath's documented syntax but is
  only validated when uploaded to SignPath. The exe's ProductVersion string is the release
  version (`0.1.0`); its numeric FileVersion is `0.1.0.0`.
- **Attestations** need a public repository (or GitHub Enterprise Cloud).
- **MSI versions** are numeric, and same-version upgrades are allowed.
