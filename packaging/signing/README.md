# Signing & Notarisation Pipeline — Operator Guide

This directory contains the scripts and policy for producing signed / notarised
release artifacts for `pcloud-rs` on macOS and Windows. Linux packages are
produced unsigned (users verify via distro repo signatures or published SHA256
sums).

The Rust rewrite lives in ``; these scripts sign the **final
packaged binaries** after build. They never modify binary contents.

---

## 1. Apple — Developer ID signing & notarisation

### 1.1 Certificate acquisition

1. Enrol in the [Apple Developer Program](https://developer.apple.com/programs/).
   Cost: **USD $99/year** (individual or organisation).
2. In the Apple Developer portal, create a **Developer ID Application**
   certificate (not "Mac App Store"). Download the `.cer`.
3. Double-click the `.cer` to install into the login Keychain, or `security
   import` it into a dedicated build keychain.
4. Export the full identity (cert + private key) as a password-protected
   `.p12` for CI use:
   ```
   security export -k login.keychain -t identities -f pkcs12 \
     -P "<password>" -o developer-id.p12
   ```

### 1.2 Local `codesign` basics

```
codesign --force --options runtime --timestamp \
  --sign "Developer ID Application: Acme Corp (TEAMID)" \
  ./path/to/binary
codesign --verify --strict --verbose=2 ./path/to/binary
spctl --assess --type execute --verbose=4 ./path/to/binary
```

- `--options runtime` enables the **hardened runtime** (required for
  notarisation).
- `--timestamp` embeds a trusted timestamp (required for notarisation).
- `--entitlements` attaches an entitlements plist (see
  `packaging/macos/entitlements.plist`).

### 1.3 CI keychain unlock

GitHub Actions macOS runners start with a locked keychain. In the workflow:

```
security create-keychain -p "$KEYCHAIN_PASSWORD" build.keychain
security default-keychain -s build.keychain
security unlock-keychain -p "$KEYCHAIN_PASSWORD" build.keychain
security set-keychain-settings -lut 21600 build.keychain
echo "$APPLE_CERT_P12_BASE64" | base64 -d > cert.p12
security import cert.p12 -k build.keychain \
  -P "$APPLE_CERT_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$KEYCHAIN_PASSWORD" build.keychain
```

### 1.4 Notarisation

Use `notarytool` (the legacy `altool` is deprecated):

```
xcrun notarytool submit ./artifact.pkg \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" \
  --wait
xcrun stapler staple ./artifact.pkg
xcrun stapler validate ./artifact.pkg
```

Generate an app-specific password at <https://appleid.apple.com/account/manage>
— **not** your primary Apple ID password.

---

## 2. Windows — EV Code Signing

### 2.1 Certificate acquisition

Windows SmartScreen reputation only builds quickly for **EV** certs. Issuers:

| CA          | Approx annual cost | One-time ID verification | Delivery                                                      |
|-------------|-------------------:|-------------------------:|---------------------------------------------------------------|
| DigiCert    | ~$600–700/yr       | ~$50                     | USB hardware token (SafeNet) **or** KeyLocker cloud HSM       |
| Sectigo     | ~$400–500/yr       | ~$50                     | USB hardware token **or** Sectigo cloud signing               |
| SSL.com     | ~$400–500/yr       | ~$50                     | USB hardware token **or** eSigner cloud HSM (CodeSignTool)    |

All three CAs now charge an additional **one-time ~USD $50** identity /
enterprise validation fee (D-U-N-S lookup, phone-verified callback, notarised
officer letter). Budget ~$450–750 year-one, ~$400–700 yearly thereafter.

**Hardware token (USB) vs cloud HSM** — pick one before ordering:

- **Hardware token (USB)**
  - Ships physical SafeNet / YubiKey FIPS token with the private key
    non-exportable. Plug-in required at sign time.
  - *Pros:* cheapest; CA default; satisfies CA/Browser Forum baseline by
    construction; FIPS 140-2 L2+.
  - *Cons:* unusable in hosted CI (GitHub Actions / Azure Pipelines cloud
    runners) without a self-hosted Windows runner that has the token
    physically attached. Token PIN entry is interactive unless the CSP is
    configured for unattended mode. Loss of the token is a hard revocation
    event.
- **Cloud HSM / cloud signing**
  - Private key is generated and held inside an HSM service (DigiCert
    **KeyLocker**, SSL.com **eSigner** / CodeSignTool, Sectigo cloud signing,
    **Azure Key Vault** with an EV cert imported via the CA's onboarding,
    **AWS CloudHSM** with a supported KSP / JSign bridge). Authentication is
    via OAuth client-credentials or KSP integration.
  - *Pros:* works natively on GitHub-hosted runners; no physical token; audit
    log; easy key rotation / revocation.
  - *Cons:* higher ongoing cost (cloud signing bundles often add
    per-signature or monthly fees); requires provider-specific tooling
    (`smctl.exe`, `CodeSignTool.jar`, `AzureSignTool`) rather than plain
    `signtool /f file.pfx`.

Standard (non-EV) OV certs can still be delivered as a plain `.pfx`
(~$200/yr) and that is what the current `build-windows` job consumes. OV
certs sign cleanly but **SmartScreen reputation warm-up** takes noticeably
longer (typically **weeks to 2–3 months** of steady installs before the "this
app might be unsafe" prompt disappears for a fresh OV-signed binary; EV
certs inherit reputation **immediately**, i.e. no warm-up).

Plan the cert swap well before a consumer-facing release: order the EV cert,
complete the ID-verification call, provision the cloud HSM, and replace
`WINDOWS_CERT_PFX_BASE64` with the provider-specific secrets before cutting
v1.0.

### 2.2 `signtool` patterns

```
signtool sign /v /fd sha256 /a /as ^
  /tr http://timestamp.digicert.com /td sha256 ^
  /f cert.pfx /p "<password>" ^
  path\to\binary.exe

signtool verify /pa /v path\to\binary.exe
```

Flag meanings:

- `/fd sha256` — file digest algorithm.
- `/td sha256` — timestamp digest algorithm.
- `/tr` — RFC 3161 timestamp server (DigiCert / Sectigo / GlobalSign).
- `/a` — auto-select best cert.
- `/as` — **append** signature (dual-sign SHA1+SHA256 for legacy OSes; we
  only ship SHA256, but `/as` is harmless for single-sig).

Sign MSI, EXE, and any DLLs bundled in the installer. Sign the **MSI after**
its payload is signed.

---

## 3. GitHub Actions secret inventory

The `.github/workflows/release.yml` workflow expects the following secrets to
be configured in the repository settings (**Settings → Secrets and variables
→ Actions**):

| Secret name                       | Purpose                                                    |
|-----------------------------------|------------------------------------------------------------|
| `APPLE_ID`                        | Apple ID email (notarytool auth).                          |
| `APPLE_APP_SPECIFIC_PASSWORD`     | App-specific password for notarytool.                      |
| `APPLE_TEAM_ID`                   | 10-char Apple Developer Team ID.                           |
| `APPLE_CERT_P12_BASE64`           | Base64-encoded Developer ID `.p12` bundle.                 |
| `APPLE_CERT_PASSWORD`             | Password for the `.p12`.                                   |
| `APPLE_KEYCHAIN_PASSWORD`         | Ephemeral build-keychain password (generated, not reused). |
| `WINDOWS_CERT_PFX_BASE64`         | Base64-encoded `.pfx` (OV certs only).                     |
| `WINDOWS_CERT_PASSWORD`           | Password for the `.pfx`.                                   |

For EV cloud signing (DigiCert KeyLocker / SSL.com eSigner / Azure Key Vault)
these PFX secrets are replaced with provider-specific credentials; update the
`sign-windows.ps1` invocation accordingly and document the replacements in a
follow-up revision of this file.

---

## 4. Reproducibility & release hygiene

- Pin `SOURCE_DATE_EPOCH` to the tag's commit timestamp in every build job:
  ```
  export SOURCE_DATE_EPOCH=$(git log -1 --pretty=%ct "$GITHUB_REF_NAME")
  ```
  Most modern toolchains honour it; `cargo` honours it transitively via
  embedded mtimes in crate archives.
- **Never modify artifact contents after signing.** Signing is a leaf step.
  If a post-sign tweak is needed, re-build, re-sign.
- Record `sha256sum` of every uploaded artifact in the GitHub Release body
  and in `release-artifacts.txt`.
- Publish `.sig` / `.pem` alongside artifacts where possible (e.g. via
  `cosign sign-blob` for Linux tarballs — out of scope for this doc, tracked
  separately).
- Treat the signing scripts as audit-surface: any change should be reviewed
  by at least one other maintainer.

---

## 5. Entitlements policy

See `packaging/macos/entitlements.plist`. The file intentionally requests the
**minimum** entitlements. Fuse-T integration may require
`com.apple.security.cs.disable-library-validation`; that entitlement is
gated behind a TODO and must be reviewed before enabling, because it weakens
the hardened runtime's DYLD protections.

---

## 6. Disaster recovery

- Keep an **offline backup** of each `.p12` / `.pfx` in a sealed envelope in
  a safe; CI secrets are not a backup.
- Record the cert thumbprint / SHA-1 in the release log so a revoked cert can
  be identified quickly.
- If a signing key is compromised: revoke at the CA, rotate the CI secret,
  re-sign and re-release affected artifacts, publish an advisory.

---

## 7. First-time notarisation runbook (Apple)

This runbook is the canonical step-by-step for moving the macOS release job
from `continue-on-error: true` + unsigned tarball to a real, stapled,
notarised `.pkg`. Execute top-to-bottom **once**, by a maintainer with admin
access to both the Apple Developer account and the GitHub repository secrets.

Estimated first-run time: ~2–4 hours of active work; budget a business day
in case Apple asks for re-verification or D-U-N-S issuance is slow.

### 7.1 Apple Developer Program enrollment

1. Create (or reuse) an Apple ID that will be the release identity. Use a
   **shared team Apple ID** (e.g. `releases@yourorg.example`) with 2FA
   enabled via a hardware key — not an individual maintainer's personal
   account.
2. Enrol that Apple ID in the [Apple Developer Program](https://developer.apple.com/programs/):
   - Organisation enrollment (preferred) requires a D-U-N-S number. Start
     this first — D-U-N-S issuance can take 1–5 business days.
   - Individual enrollment is instant but ties the cert to one person; not
     recommended for a shared project.
   - Cost: USD $99 / year.
3. Record the 10-character **Team ID** from
   <https://developer.apple.com/account> → "Membership details". This is
   the value of the `APPLE_TEAM_ID` CI secret.

### 7.2 Create the Developer ID Application certificate in Xcode

1. On a clean macOS machine: install Xcode (App Store) → launch once → accept
   the license.
2. Xcode → Settings → Accounts → add the release Apple ID → select the team →
   click **Manage Certificates…** → `+` → **Developer ID Application**.
   - Choose "Developer ID Application", **not** "Apple Development",
     "Mac Installer", or "Mac App Store". The hardened runtime + notarisation
     pipeline requires exactly the Application variant.
   - The private key is generated locally in the login keychain.
3. Open **Keychain Access** → login keychain → Certificates → locate
   "Developer ID Application: <Your Org> (TEAMID)". Verify:
   - The certificate has a disclosure triangle showing a matching private key
     beneath it. If not, the key never made it to this machine and signing
     will fail at `codesign` time.
   - Trust is `Always Trust` or inherits Apple Root CA trust.

### 7.3 Export the .p12 for CI use

1. In Keychain Access, **select both** the certificate and its private key
   (Cmd-click). Right-click → **Export 2 items…**.
2. File format: **Personal Information Exchange (.p12)**.
3. Set a strong passphrase (24+ random chars). This is the value of the
   `APPLE_CERT_PASSWORD` CI secret.
4. Base64-encode for secret storage:
   ```
   base64 -i DeveloperID.p12 -o DeveloperID.p12.b64
   ```
   The file contents go into `APPLE_CERT_P12_BASE64`.
5. **Seal the original `.p12` and passphrase in offline backup** (see §6).
   The CI secret is not a backup.

### 7.4 Generate the app-specific password for notarytool

`notarytool` cannot use your primary Apple ID password, and it cannot use a
session-scoped Xcode token — it needs an **app-specific password**.

1. Go to <https://appleid.apple.com/account/manage> → sign in as the release
   Apple ID → **App-Specific Passwords** → `+` → label it
   `pcloud-rs-notarytool-ci`.
2. Copy the generated password (Apple shows it exactly once). This is the
   value of the `APPLE_APP_SPECIFIC_PASSWORD` CI secret.
3. If you ever need to rotate it: revoke the old label in the same screen
   and re-issue. Always rotate on maintainer offboarding.

### 7.5 Generate the ephemeral keychain password

The `APPLE_KEYCHAIN_PASSWORD` secret is not a long-lived credential — it
creates a fresh `build.keychain` on each CI run and unlocks it for
`codesign`. Generate ~24 random bytes:
```
openssl rand -base64 24
```
and paste as the `APPLE_KEYCHAIN_PASSWORD` secret.

### 7.6 Populate GitHub Actions secrets

Under **Repository → Settings → Secrets and variables → Actions → New
repository secret**, create (names must match §3 exactly):

| Secret                          | Value                                    |
|---------------------------------|------------------------------------------|
| `APPLE_ID`                      | Release Apple ID email                   |
| `APPLE_TEAM_ID`                 | 10-char Team ID from §7.1                |
| `APPLE_CERT_P12_BASE64`         | Base64 of the .p12 from §7.3             |
| `APPLE_CERT_PASSWORD`           | .p12 passphrase from §7.3                |
| `APPLE_APP_SPECIFIC_PASSWORD`   | App-specific password from §7.4          |
| `APPLE_KEYCHAIN_PASSWORD`       | Random keychain password from §7.5       |

Do **not** put any of these in repo variables (which are readable in logs),
environment files, or repo-level non-secret settings.

### 7.7 Dry-run release

1. Push a pre-release tag, e.g. `v0.0.0-rc1`, so the signing job runs end to
   end without announcing a production version.
2. Watch the `build-macos` job. Expected step sequence:
   `Preflight signing scripts` → `Preflight secrets` (have_secrets=true) →
   `Build universal binaries` → `Import signing certificate` →
   `Discover signing identity` → `Sign pcloud-rs and pcloudd` →
   `Build pkg installer` → `Notarise and staple`.
3. Download the resulting `.pkg` from the workflow artifacts. On a macOS
   machine:
   ```
   spctl --assess --type install --verbose=4 pcloud-rs-0.0.0-rc1.pkg
   xcrun stapler validate               pcloud-rs-0.0.0-rc1.pkg
   pkgutil --check-signature            pcloud-rs-0.0.0-rc1.pkg
   ```
   All three must pass cleanly.
4. Flip `continue-on-error: false` on the `build-macos` job, commit, then tag
   the real release.

### 7.8 Build-log secret scanning

After the first successful run, audit the workflow logs before declaring the
pipeline hardened:

1. Download the raw logs from the GitHub Actions run.
2. Scan for high-risk strings:
   ```
   rg -n 'BEGIN (RSA |EC |ENCRYPTED )?PRIVATE KEY' workflow.log
   rg -n 'MIIK|MIIJ|MIIH'                          workflow.log   # .p12 PEM prefixes
   rg -n 'App-Specific-Password|xxxx-xxxx'         workflow.log
   rg -nE '[A-Z0-9]{10}\)'                         workflow.log   # Team ID parenthetical
   ```
   The only acceptable Team ID occurrences are the `identity=…` line and the
   `Developer ID Application: … (TEAMID)` codesign log lines. Anything else
   (especially inside a `security import` or `base64 -d` echo) means a secret
   leaked into stdout — rotate immediately via §7.9.
3. Confirm GitHub's automatic log masking is working: every occurrence of a
   secret value that _does_ surface must render as `***`.
4. If a leak is confirmed, treat the cert as compromised and follow §7.9.

### 7.9 Rollback plan if notarisation rejects

Apple can reject a submission for hardened-runtime violations, missing
timestamps, entitlement mismatches, or unsigned nested binaries. When
`notarytool submit --wait` returns status `Invalid`:

1. Pull the detailed rejection log immediately:
   ```
   xcrun notarytool log <submission-id> \
     --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
     --password "$APPLE_APP_SPECIFIC_PASSWORD" rejection.json
   ```
   Each entry has `path`, `message`, and often a `docUrl` to the exact Apple
   TN.
2. **Do not** publish the unnotarised artifact. With `continue-on-error:
   false` the `build-macos` job will already have failed the release; if any
   artifacts reached the GitHub Release draft, delete them.
3. Common rejection causes and fixes:
   - `The signature of the binary is invalid` → re-sign with
     `--options runtime --timestamp`; inspect the entitlements plist for
     drift.
   - `The binary uses an SDK older than the 10.9 SDK` → rebuild on the
     pinned `macos-14` runner with a current Rust toolchain.
   - `The executable does not have the hardened runtime enabled` → missing
     `--options runtime`; inspect codesign output.
   - `The signature does not include a secure timestamp` → Apple's timestamp
     server was unreachable; re-run the job.
4. If the cert itself is flagged as revoked or compromised:
   a. Revoke the certificate in the Apple Developer portal immediately.
   b. Rotate `APPLE_CERT_P12_BASE64` and `APPLE_CERT_PASSWORD` to empty
      placeholders so `have_secrets=false` flips the job back to the
      unsigned-fallback path (prevents broken signed releases while you
      remediate).
   c. Repeat §7.2 – §7.3 to mint a new Developer ID Application cert, then
      §7.6 to refresh the secrets.
   d. Publish a security advisory listing the artifact SHA256 values signed
      by the revoked cert so downstream packagers can invalidate them.
5. Post-mortem: record the rejection reason, resolution, and any workflow
   changes in the release notes and in a new bead under `bd-1du.10` so the
   parity-proof evidence trail stays honest.
