# Install DataDeck on macOS

When a tagged release is available, download the disk image for your Mac. Before the first signed release, you can [build and run DataDeck from source](../README.md#run-on-macos). Choose **arm64** if About This Mac lists an Apple chip, or **x86_64** if it lists an Intel processor. Release builds require macOS 15 or later. Open the disk image, drag **DataDeck** into **Applications**, then eject the image and launch DataDeck. Public downloads carry a Developer ID signature and an Apple notarization ticket. [DataDeck releases](https://github.com/datasets/datadeck/releases)

On a Mac with the default Gatekeeper settings, a signed and notarized release opens with the normal first-launch confirmation for internet downloads. Users can keep their security settings enabled. Managed Macs may have additional installation policies. [Apple’s guide to opening downloaded apps](https://support.apple.com/en-us/102445)

To verify a download, save `SHA256SUMS` and your DMG in the same folder. Run the command below there. The downloaded image should show `OK`.

```sh
shasum -a 256 --ignore-missing -c SHA256SUMS
```

## Preview a build

From **Actions → macOS release → Run workflow**, choose the branch to test. Each native Mac builds DataDeck, runs its release-mode test suite, mounts the resulting image, and runs a small SQL recipe from the bundled executable. GitHub-hosted previews require macOS 15 or later. Download the artifact for the CPU you need and unzip it to find the DMG and its checksum. Preview filenames end in `-unsigned-preview.dmg`. They carry a local ad-hoc signature, so Gatekeeper can block a downloaded preview. Preview runs use no Apple credentials and produce workflow artifacts for 14 days. [Run a preview](https://github.com/datasets/datadeck/actions/workflows/release-macos.yml)

For a local preview on a Mac with Rust and Xcode Command Line Tools installed, run these commands from the checkout. Local builds inherit the toolchain’s deployment target.

```sh
scripts/package-macos.sh
scripts/macos-dmg.sh preview target/release/DataDeck.app "$(uname -m)" /tmp/datadeck-preview
```

## Set up public release signing

1. Enroll the publishing individual or organization in the Apple Developer Program. Apple lists its annual membership at US$99 or local equivalent and provides fee waivers for eligible organizations. The Account Holder creates a **Developer ID Application** certificate using a certificate signing request from a trusted Mac. Install the certificate where its matching private key was generated. In Keychain Access, export the signing identity (certificate and private key) as a password-protected `.p12`. Choose the modern G2 certificate for a new identity. CI can install the checksum-verified G2 intermediate from Apple when the runner needs it. Include the issuing intermediate when exporting a legacy identity. [Apple enrollment](https://developer.apple.com/help/account/membership/program-enrollment/) · [Create a Developer ID certificate](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/) · [Apple certificate authorities](https://www.apple.com/certificateauthority/)
2. Enable App Store Connect API access for the team. Under **Users and Access → Integrations → App Store Connect API → Team Keys**, an Account Holder or Admin generates a dedicated key for notarization. Use the least privileged role your team permits for notarization. Record its key ID and issuer ID, download the private `.p8` once, and keep the original in your encrypted secret store. Team keys can cover multiple apps on the same team. [Create and manage App Store Connect keys](https://developer.apple.com/help/app-store-connect/get-started/app-store-connect-api/)
3. Create a GitHub Actions environment named `macos-release`. Restrict deployment **tags** to `v*`, and configure repository rules so only authorized maintainers can create, update or delete matching tags. A tagged commit runs its own build and signing scripts. Add the five environment secrets below. Keep `macos-preview` free of signing credentials. [Manage environment protection and secrets](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments) · [GitHub rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)

| Environment secret | Value |
| --- | --- |
| `APPLE_DEVELOPER_ID_P12_BASE64` | Base64 encoding of the password-protected signing identity export. |
| `APPLE_DEVELOPER_ID_P12_PASSWORD` | Password used to export that identity. |
| `APPLE_NOTARY_KEY_BASE64` | Base64 encoding of the downloaded App Store Connect team private key file. |
| `APPLE_NOTARY_KEY_ID` | Key ID belonging to that private key. |
| `APPLE_NOTARY_ISSUER_ID` | Team issuer UUID from App Store Connect. |

Stream encoded files directly into the environment secret store to keep their contents off the terminal. For example, from the repository checkout on a trusted Mac, the following command sends the signing identity to GitHub. Use the same pattern for the private API key. Use `gh secret set --env macos-release NAME` with interactive entry for the password and IDs. Keep private key material out of Git, artifacts and logs.

```sh
/usr/bin/base64 -i /secure/path/DataDeck.p12 | gh secret set APPLE_DEVELOPER_ID_P12_BASE64 --env macos-release
/usr/bin/base64 -i /secure/path/AuthKey.p8 | gh secret set APPLE_NOTARY_KEY_BASE64 --env macos-release
```

CI imports the signing identity into a temporary per-job keychain. The app receives a secure timestamp and hardened runtime with no additional entitlements. The workflow signs the outer DMG, submits it using `notarytool`, waits for Apple to return `Accepted`, then staples and validates the ticket. Apple generates tickets for the signed app inside the image as well. CI assesses the final DMG and mounted app with Gatekeeper and exercises the exact executable users receive. Local credential files and the temporary keychain are removed at job completion. [Apple notarization requirements](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) · [Apple’s packaging and stapling workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)

## Publish a version

Update the package version in `Cargo.toml`, refresh `Cargo.lock`, and merge the release commit. Push an annotated tag matching `vMAJOR.MINOR.PATCH` exactly. The following example publishes version 0.2.0.

```sh
git tag -a v0.2.0 -m "DataDeck 0.2.0"
git push origin v0.2.0
```

A tag starts two builds on GitHub-hosted macOS 15 runners, one for each CPU. The packaging job checks the version, CPU architecture, minimum macOS version, signature, system library dependencies, icon, installation shortcut and SQL output in each mounted disk image. A missing credential, rejected notarization or failure on either CPU blocks publication. Once both jobs pass, the publisher verifies both SHA-256 digests, uploads both images and `SHA256SUMS` to a draft, then makes it public. A failed upload leaves a draft that the same workflow can resume on rerun. A completed public release is left intact.

For a certificate or key rotation, replace the corresponding `macos-release` secrets and keep the old credentials until the new ones are verified, unless revocation is needed for compromise. For a notarization rejection, inspect the submission ID and Apple diagnostic log in the failed job. GitHub has announced the retirement of its macOS 15 Intel runners for fall 2027. Maintaining native Intel verification beyond that point will require a replacement runner. [Apple notarization diagnostics](https://developer.apple.com/documentation/security/resolving-common-notarization-issues) · [GitHub Intel runner notice](https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/)
