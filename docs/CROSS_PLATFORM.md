# Cross-platform direction

The product can remain free and open source under the repository's MIT License
on Windows, macOS, and Linux. Platform distribution credentials are separate
from the source-code license, and every bundled model, dictionary, runtime, and
installer dependency needs its own recorded license.

## Shared product layer

Keep these components platform-neutral:

- detector, language data, layout transformation, snippets, and user learning;
- versioned commands and status contracts;
- speech orchestration, model manifests, translation routing, and privacy
  policy;
- settings schema and most of the tray/settings presentation model.

Replace WPF with the MIT-licensed Avalonia framework when the shared desktop UI
work begins. Do not use the separately licensed commercial Avalonia tooling or
XPF runtime. The existing WPF UI can remain the Windows shell until the
cross-platform surface is ready.

There is deliberately no trait layer for this yet. One existed — a
`platform-api` crate of capability traits — and nothing ever implemented or
called it: every caller went straight to the Windows implementation, so the
crate promised a portability the code did not have. It has been removed. The
seam belongs in the commit that introduces a second platform, shaped by what
that platform actually needs rather than by what one guesses in advance.

Each operating system gets a small native platform host implementing:

```text
global shortcut / push-to-talk
audio capture
foreground target identity
selected-text read and replace
guarded clipboard access
text insertion
permissions and secure-field policy
tray / autostart / update integration
```

Whisper.cpp and the text-translation workers remain local cross-platform
services. Model downloads stay optional and are verified through a signed
manifest and SHA-256.

## macOS

Use a Swift or Objective-C++ host around `CGEventTap`, Accessibility APIs,
Core Audio, and `NSPasteboard`. Request Microphone, Accessibility, and Input
Monitoring permissions explicitly. Build arm64 and x64 artifacts, or a universal
application bundle.

Development and source builds can be free. A normal downloaded application that
opens without Gatekeeper friction requires Apple Developer Program membership,
a Developer ID signature, hardened runtime, and notarization. Until a paid
Developer ID is available, publish reproducible unsigned source builds with
clear local-build instructions; do not present them as frictionless public
installers.

## Linux

Support is capability-based rather than a single universal implementation:

1. X11: full global shortcuts, selection conversion, clipboard insertion, and
   dictation through X11/XInput/XTest and AT-SPI where available.
2. KDE Plasma and GNOME on Wayland: use the XDG GlobalShortcuts portal for
   push-to-talk and portal/compositor-approved clipboard or input mechanisms.
3. Other Wayland compositors: detect portal support and expose an honest
   compatibility status. Allow compositor-specific bindings or a manual
   clipboard mode when universal insertion is unavailable.

PipeWire is the preferred Wayland-era audio path, with PulseAudio/ALSA fallback
where needed. Native `.deb` and `.rpm` packages should be the first distribution
formats because unrestricted desktop integration is central to the product.
Flatpak can follow after its portal behavior is proven. AppImage can be offered
as a portable secondary package.

## Delivery order

1. Extract portable application services and add platform capability contracts.
2. Introduce an Avalonia tray/settings shell without replacing the stable
   Windows input host.
3. Build and test the macOS host on real Intel and Apple Silicon hardware.
4. Build Linux X11 support and an automated compatibility matrix.
5. Add GNOME and KDE Wayland portal backends, then document compositor-specific
   limitations.
6. Add per-platform packaging, update manifests, dependency notices, and CI.

Cross-compilation can catch compile-time failures, but microphone permissions,
global shortcuts, Accessibility/AT-SPI, text insertion, Gatekeeper, and Wayland
portal behavior require real machines or CI runners on each target platform.
