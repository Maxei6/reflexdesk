# Release security

ReflexDesk preview installers are intentionally unsigned.

A **stable public release must not be published** until all platform signing and updater credentials are configured.

## Required external credentials

### Windows
- trusted code-signing certificate
- certificate/private-key access configured only in GitHub Actions secrets or a secure signing service
- SmartScreen reputation strategy

### macOS
- Apple Developer ID Application certificate
- certificate password
- Apple ID / app-specific password or App Store Connect API credentials
- Team ID
- notarization and stapling verified in CI

### Updater
- Tauri updater private signing key stored only as a secret
- updater public key embedded in the application
- stable update metadata endpoint / GitHub release feed

## Policy

- pull requests and main-branch preview builds may remain unsigned
- tagged preview releases stay draft/prerelease
- stable releases require a dedicated signed-release workflow
- private keys are never committed to this repository
- the update client must fail closed on invalid signatures
- pinned CrispASR runtime archives remain SHA-256 verified independently of app-update signing; model-weight integrity follows the selected upstream resolver and model distribution

## Current state

The cross-platform build pipelines are active and tested.

Code-signing, notarization and automatic updater activation are **externally blocked**
until the owner provides the platform credentials above. This is intentional:
ReflexDesk will not invent, generate, or commit production signing identities.
