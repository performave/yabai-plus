# CI setup

One-time setup for the [`release` workflow](../.github/workflows/release.yml).
After that, releasing is just [pushing a tag](./releasing.md).

## Signing modes

The **`MACOS_SIGNING`** repository variable picks how a release is signed. Set
it under **Settings → Secrets and variables → Actions → Variables**, not under
Secrets.

| `MACOS_SIGNING` | signature | needs | Accessibility after `brew upgrade` |
| --- | --- | --- | --- |
| unset, or `adhoc` | ad-hoc, no certificate | nothing | must be granted again, every release |
| `signed` | your certificate, not notarized | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | kept |
| `notarized` | Developer ID, notarized | the above with a Developer ID, plus `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` | kept |

```bash
gh variable set MACOS_SIGNING --body signed     # or notarized
gh variable delete MACOS_SIGNING                # back to adhoc
```

**There is no paid Apple Developer account right now**, so the variable is
`signed`, and the certificate secrets hold the free account's Apple Development
certificate (`2RTC9978WY`). The App Store Connect secrets left over from the
paid team (8UR4G77744, now revoked) are not read in that mode. When there is a
paid account again, update the secrets (sections 1 and 2) and set the variable
to `notarized`. The workflow needs no edit.

TCC records the Accessibility grant against the binary's designated requirement.
An ad-hoc signature's requirement is the binary's own hash, which every release
changes. A certificate gives a requirement that holds across releases, and a
free account's Apple Development certificate is enough for that. Notarization
only matters to Gatekeeper, which checks only quarantined files. Homebrew does
not quarantine what a formula installs.

## Prerequisites

- Admin access to the GitHub repo, to add Actions secrets and variables.
- For `signed`: any code-signing certificate. A free Apple ID's **Apple
  Development** certificate works (Xcode → Settings → Accounts → Manage
  Certificates → + → Apple Development).
- For `notarized`: an **Apple Developer Program** membership. It is needed for a
  "Developer ID Application" certificate and for notarization.

## 1. Signing certificate (`signed`, `notarized`)

This is the certificate the binary is signed with. For `notarized` it must be a
**Developer ID Application** certificate. For `signed`, use the free account's
Apple Development one instead, exported the same way.

1. In **Keychain Access** (or via Xcode → Settings → Accounts → Manage
   Certificates), create/download a **Developer ID Application** certificate.
2. Export it as a `.p12`: select the certificate **and** its private key →
   right-click → Export → `.p12`, and set an export password.
3. Note the exact identity name for later:
   ```bash
   security find-identity -v -p codesigning
   # -> "Developer ID Application: Your Name (TEAMID)"
   ```

## 2. App Store Connect API key (`notarized` only)

Using an API key avoids the 2FA/session flakiness of Apple-ID + app-specific
passwords.

1. Go to <https://appstoreconnect.apple.com> → **Users and Access** →
   **Integrations** → **App Store Connect API** → generate a key with the
   **Developer** role.
2. Record the **Key ID** and **Issuer ID** shown on that page.
3. Download the `AuthKey_XXXXXXXX.p8` file. **You can only download it once.**

## 3. GitHub Actions secrets

Add these under **Settings → Secrets and variables → Actions** on the repo. The
names intentionally match the `easel` repo, so the same values can be reused.

| Secret | Mode | Value |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | `signed`, `notarized` | base64 of the `.p12`: `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | `signed`, `notarized` | the password set when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | `signed`, `notarized` | full identity, e.g. `Developer ID Application: Your Name (TEAMID)` or `Apple Development: you@example.com (XXXXXXXXXX)` |
| `APPLE_API_KEY` | `notarized` | App Store Connect **Key ID** |
| `APPLE_API_ISSUER` | `notarized` | App Store Connect **Issuer ID** (a UUID) |
| `APPLE_API_PRIVATE_KEY` | `notarized` | raw contents of `AuthKey_XXXX.p8` (paste the PEM text, including the BEGIN/END lines) |

Scripting it instead of clicking:

```bash
gh secret set APPLE_CERTIFICATE          < <(base64 -i cert.p12)
gh secret set APPLE_CERTIFICATE_PASSWORD --body 'the-p12-password'
gh secret set APPLE_SIGNING_IDENTITY     --body 'Developer ID Application: Your Name (TEAMID)'
gh secret set APPLE_API_KEY              --body 'ABC123DEF4'
gh secret set APPLE_API_ISSUER           --body '00000000-0000-0000-0000-000000000000'
gh secret set APPLE_API_PRIVATE_KEY      < AuthKey_ABC123DEF4.p8
```

## 4. Why these specific choices

These mirror a setup that's known to work and sidestep common notarization
failures:

- **A variable, not secret presence, picks the mode.** Secrets left over from a
  lapsed account are still "set", and `codesign` signs with a revoked
  certificate without complaint.
- **`apple-actions/import-codesign-certs`** for the cert import — it creates a
  temporary `signing_temp` keychain, imports the `.p12`, and sets the key
  partition list so `codesign` works non-interactively (the latter being a common
  failure when done by hand on recent macOS runners).
- **App Store Connect API key** rather than Apple-ID auth — no 2FA prompts in CI.
- **`.p8` stored as raw text**, written with `printf` — avoids a base64
  round-trip getting mangled.

## 5. Verify

1. Confirm the variable (`gh variable list`; nothing means `adhoc`) and the
   secrets that mode needs (`gh secret list`).
2. Trigger a run: either push a `v*` tag, or use the **workflow_dispatch** button
   on the Actions tab.
3. **Choose signing mode** prints the mode, and **Codesign** prints the
   `Authority=` lines it signed with (none for ad-hoc).
4. Under `notarized`, watch the **Notarize** step — `notarytool ... --wait`
   blocks until Apple returns `Accepted` (or a rejection with a log URL).

## Troubleshooting

| Symptom | Likely cause |
| --- | --- |
| "MACOS_SIGNING is '…'" | the variable holds something other than `adhoc`, `signed` or `notarized` |
| "needs the APPLE_SIGNING_IDENTITY secret" | `signed`/`notarized` set without the certificate secrets |
| `codesign --verify`: `CSSMERR_TP_CERT_REVOKED` | the certificate was revoked, as the lapsed paid team's was. `codesign` still signs with it, but macOS kills the binary at launch; switch to `adhoc` or a valid certificate |
| cert import step fails | bad `APPLE_CERTIFICATE` base64 (re-export, use `base64 -i`) or wrong `APPLE_CERTIFICATE_PASSWORD` |
| `codesign` errors with "no identity found" | `APPLE_SIGNING_IDENTITY` doesn't match `security find-identity` output exactly |
| notarytool `Invalid` | binary not signed with `--options runtime` / `--timestamp`, or signed with a non-Developer-ID cert |
| notarytool auth error | wrong Key ID / Issuer ID, the `.p8` text is incomplete, or no active Apple Developer Program membership |
| Release not created | tag didn't match `v*`, or `contents: write` permission missing |
