# Microsoft Store submission for SayFlex

SayFlex can produce a Store-oriented MSIX from the same staged payload as the
offline installer. Packaging is an engineering prerequisite, not Store
authorization: Microsoft accepts and signs the package only after the product
owner completes Partner Center enrollment and the submission passes
certification.

## Owner-only prerequisites

The product owner must do these steps in Partner Center because they create
legal and publisher identity that cannot be delegated to a build script:

1. Open and verify a Windows developer account and accept the current
   agreements.
2. Reserve the SayFlex product name.
3. Copy the exact package `Identity/Name`, `Identity/Publisher`, and publisher
   display name assigned by Partner Center. They must not be guessed.
4. Supply the Store listing: descriptions, support contact, public privacy
   policy URL, age rating answers, screenshots and markets.
5. Review capability declarations and submit the package for certification.

Microsoft's current onboarding and first-publication documentation:

- <https://learn.microsoft.com/windows/apps/publish/partner-center/open-a-developer-account>
- <https://learn.microsoft.com/windows/apps/package-and-deploy/publish-first-app>

## Build the package

First create the exact staged release in `dist/app`. Then use the values copied
from Partner Center:

```powershell
pwsh -File scripts/build-msix.ps1 `
  -IdentityName '<Partner Center package identity name>' `
  -Publisher '<Partner Center publisher distinguished name>' `
  -PublisherDisplayName '<public publisher name>'
```

This produces `dist/SayFlex-<version>-win-x64-store-unsigned.msix` and a SHA-256
sidecar. The script verifies that every staged payload file is present with the
same size and hash, rejects additional payload files, and binds the MSIX
identity and four-part version to `build-manifest.json`.

For a packaging-only experiment, use an explicitly non-production identity:

```powershell
pwsh -File scripts/build-msix.ps1 -DevelopmentIdentity
```

The resulting `development-unsigned.msix` is deliberately named so it cannot
be mistaken for a Partner Center artifact. A local Windows installation also
needs a certificate trusted on that machine; do not distribute the test
certificate or package.

MSIX signing behavior is described by Microsoft here:

- <https://learn.microsoft.com/windows/msix/package/signing-package-overview>
- <https://learn.microsoft.com/windows/apps/publish/faq/get-started-with-the-microsoft-store>

## Required live checks before submission

Packaging success does not prove that a full-trust desktop app behaves the same
inside package identity. Install a signed test package and exercise:

- global dictation and translation hotkeys in editor, browser and terminal;
- microphone capture and all CPU/CUDA/Vulkan worker launches;
- local translation runtime launch and shutdown;
- cloud speech and translation requests;
- Companion window creation, movement and persistence;
- model download, engine unload, deletion and reinstall;
- settings/history/credential persistence across package update;
- uninstall behavior and preservation/removal choices for user data.

Keep the current Inno Setup package as a separate distribution channel until
this matrix passes. Store certification signs an MSIX/AppX submission; it does
not sign the existing direct-download EXE installer. Microsoft currently
requires EXE/MSI submissions to use an immutable HTTPS URL, support unattended
offline installation and carry a valid digital signature on the installer and
all portable executables:

- <https://learn.microsoft.com/windows/apps/publish/publish-your-app/msi/app-package-requirements>

Therefore the direct installer is not public-ready until an Authenticode
certificate or managed signing service is connected to `publish.ps1` and
`build-installer.ps1`.
