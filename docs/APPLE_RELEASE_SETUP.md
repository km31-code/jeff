# Jeff — Apple Developer Setup and First Release

Complete this doc in order after signing up for the Apple Developer Program.
Everything in the codebase is already built and tested. This is purely
account setup and secret configuration.

---

## Step 1: Join Apple Developer Program

URL: https://developer.apple.com/programs/enroll/
Cost: $99/year
Time: usually approved within 24–48 hours after payment

Use the same Apple ID you use for your Mac.

---

## Step 2: Create a Developer ID Application certificate

This is the certificate that signs Jeff so macOS Gatekeeper passes.

1. Open Keychain Access on your Mac.
2. Menu: Keychain Access → Certificate Assistant → Request a Certificate From
   a Certificate Authority.
3. Enter your email, select "Saved to disk", click Continue. Save the .certSigningRequest file.
4. Go to https://developer.apple.com/account/resources/certificates/add
5. Select "Developer ID Application" → Continue.
6. Upload the .certSigningRequest file → Continue.
7. Download the resulting .cer file.
8. Double-click the .cer file — it installs into Keychain Access.

To verify it installed: open Keychain Access, go to "My Certificates",
look for "Developer ID Application: [Your Name] (TEAMID)".

---

## Step 3: Export the certificate as a .p12 file

The CI pipeline needs the certificate in base64-encoded .p12 format.

1. In Keychain Access → My Certificates, find "Developer ID Application: [Your Name]".
2. Right-click → Export.
3. Save as `jeff_certificate.p12`.
4. Set a strong password — you will need this password in Step 6.
5. Base64-encode it:
   ```bash
   base64 -i jeff_certificate.p12 -o jeff_certificate.b64
   ```
6. Open `jeff_certificate.b64` in a text editor — copy the full contents.
   This is your `APPLE_CERTIFICATE` secret value.

Delete `jeff_certificate.p12` and `jeff_certificate.b64` after you have
copied the values into GitHub secrets. Do not commit either file.

---

## Step 4: Collect your signing identity string

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

The output looks like:
```
1) XXXXXXXXXX "Developer ID Application: Your Name (ABCDE12345)"
```

Copy the full quoted string including "Developer ID Application:".
This is your `APPLE_SIGNING_IDENTITY` secret value.

---

## Step 5: Collect your Team ID

1. Go to https://developer.apple.com/account
2. Look for "Team ID" in the top-right membership section.
   It is a 10-character alphanumeric string, e.g. `ABCDE12345`.
3. This is your `APPLE_TEAM_ID` secret value.

---

## Step 6: Create an app-specific password for notarytool

The CI pipeline uses `xcrun notarytool` to submit the app to Apple for notarization.
It needs an app-specific password (not your main Apple ID password).

1. Go to https://appleid.apple.com/account/manage
2. Sign in → App-Specific Passwords → Generate Password.
3. Label it "jeff-notarytool" or similar.
4. Copy the generated password (shown only once).
5. This is your `APPLE_APP_PASSWORD` secret value.

---

## Step 7: Get your provider short name

This is the short name Apple uses for your developer account in App Store Connect.

1. Go to https://appstoreconnect.apple.com
2. Sign in → Users and Access → your account name.
3. The "Provider" field shows your provider short name.
   It is usually similar to your Team ID or company name abbreviation.
4. This is your `APPLE_PROVIDER_SHORT_NAME` secret value.

Alternatively, if you have the Apple Developer CLI tools, run:
```bash
xcrun altool --list-providers -u "your@apple.id" -p "@keychain:AC_PASSWORD"
```

---

## Step 8: Configure GitHub Actions secrets

Go to: https://github.com/km31-code/jeff/settings/secrets/actions

Click "New repository secret" for each of the following.
Copy the exact secret names — they are referenced by name in release.yml.

| Secret name                 | Where to find it                                    |
|-----------------------------|-----------------------------------------------------|
| `APPLE_CERTIFICATE`         | Contents of jeff_certificate.b64 (Step 3)           |
| `APPLE_CERTIFICATE_PASSWORD`| The password you set when exporting the .p12 (Step 3)|
| `APPLE_SIGNING_IDENTITY`    | Full string from Step 4 (include "Developer ID...")  |
| `APPLE_ID`                  | Your Apple ID email address                         |
| `APPLE_APP_PASSWORD`        | App-specific password from Step 6                   |
| `APPLE_TEAM_ID`             | 10-character team ID from Step 5                    |
| `APPLE_PROVIDER_SHORT_NAME` | Provider short name from Step 7                     |
| `TAURI_PUBLIC_KEY`          | See "Updater keypair" section below                 |
| `TAURI_PRIVATE_KEY`         | See "Updater keypair" section below                 |
| `TAURI_KEY_PASSWORD`        | Leave empty (no password was set on the keypair)    |

---

## Updater keypair (already generated)

The keypair was generated on 2026-04-25. Store these values as secrets.
Do not commit them to the repository.

**TAURI_PUBLIC_KEY:**
```
dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDNDMDY0OUIwRjQ5M0REMEQKUldRTjNaUDBzRWtHUEhWUFloMW9DS1h3dzBTMjZSUGl1eEoveWNRdmM4QUVNVExnKytZK0ZZZ3EK
```

**TAURI_PRIVATE_KEY:**
```
dW50cnVzdGVkIGNvbW1lbnQ6IHJzaWduIGVuY3J5cHRlZCBzZWNyZXQga2V5ClJXUlRZMEl5anpCNmUzZ3NOaEZ5TEs0OVh3WC9YSldWZXBzRDI4RHAreU4rb1ExSW5md0FBQkFBQUFBQUFBQUFBQUlBQUFBQUFYRnh4eDJML3ZuaE5VZCtkSDExQU55aVVpaEJyT1pYY2kyTURqZjlpUHR4S09GZzh0Z3o2amJwSm9KbngydS95dm56TFJaQWsxMUtHZW5NRVdFc091Z0tCY1hBa2V2aFd0aVBWVjFXaSt6UjVDdHg2T2diekZBZ2tRbHdqdjJjdU12NThBWnRyVlU9Cg==
```

**TAURI_KEY_PASSWORD:** (empty — leave the secret value blank)

---

## Step 9: Enable write permissions for GitHub Actions

The release job creates a GitHub Release using the `gh` CLI. It needs
write access to repository contents.

1. Go to https://github.com/km31-code/jeff/settings/actions
2. Scroll to "Workflow permissions".
3. Select "Read and write permissions".
4. Click Save.

---

## Step 10: Create the release branch and trigger CI

Once all 10 secrets are configured:

```bash
git push origin master:release
```

This creates the `release` branch and immediately triggers the CI pipeline.

---

## Step 11: Monitor the CI pipeline

Go to: https://github.com/km31-code/jeff/actions

You will see a workflow run called "release" with 5 sequential jobs:

1. **test** (~3 min): cargo test + phase17 regression checks + frontend tests.
   Must pass before build starts.

2. **build** (~8 min): compiles universal binary (arm64 + x86_64). Uploads
   unsigned .app as a workflow artifact.

3. **sign** (~2 min): imports your Developer ID certificate, runs codesign
   with hardened runtime and entitlements. Creates signed .dmg.

4. **notarize** (~5–15 min): submits .dmg to Apple notary service and waits
   for approval. Apple's servers add a ticket to the .dmg so Gatekeeper
   accepts it offline. This step can take up to 15 minutes.

5. **release** (~2 min): signs the updater archive with the Tauri keypair,
   generates latest.json, creates a GitHub Release at
   https://github.com/km31-code/jeff/releases with:
   - Jeff_0.1.0.dmg (the distributable installer)
   - Jeff_0.1.0_universal.app.tar.gz (the auto-update archive)
   - latest.json (the update feed)

If any job fails, click into it to see the error. Common issues and fixes
are listed in the troubleshooting section below.

---

## Step 12: Verify the release

After all 5 jobs pass:

1. Go to https://github.com/km31-code/jeff/releases
2. Download Jeff_0.1.0.dmg to your Mac.
3. Double-click the .dmg — you should see a drag-to-Applications window.
4. Drag Jeff to Applications.
5. Double-click Jeff in Applications — it should open without any
   "unidentified developer" warning. Gatekeeper accepts it cleanly.

If that works, the release is distributable. Share the .dmg download link
with anyone and it will install without friction.

---

## Troubleshooting

**sign job fails: "no identity found"**
The APPLE_SIGNING_IDENTITY secret does not match the imported certificate.
Verify the string exactly matches the output of:
```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

**notarize job fails: "invalid credentials"**
APPLE_ID or APPLE_APP_PASSWORD is wrong. Regenerate the app-specific
password at https://appleid.apple.com and update the secret.

**notarize job fails: "team not found"**
APPLE_TEAM_ID is wrong. Double-check at https://developer.apple.com/account.

**release job fails: "Resource not accessible by integration"**
Workflow write permissions are not enabled. Go to Step 9 and enable them.

**build job fails: "TAURI_PUBLIC_KEY is empty"**
The TAURI_PUBLIC_KEY secret is not set or is empty. Re-enter it from the
"Updater keypair" section above.

---

## After the first release: ongoing releases

For every subsequent release:

1. Bump the version in `desktop/src-tauri/Cargo.toml` (the `version =` field).
2. The same version must match in `desktop/src-tauri/tauri.conf.json`
   (the `"version"` field at the top).
3. Commit and push to master.
4. Push to the release branch: `git push origin master:release`
5. CI builds, signs, notarizes, and publishes the new release automatically.
6. Users already running Jeff will see the "update available" dialog on
   their next launch.
