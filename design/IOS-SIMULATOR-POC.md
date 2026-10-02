# iPhone Simulator embedding POC

This macOS experiment embeds the screen of the most recently used,
already-booted iPhone Simulator in a ket tab. It is in every macOS build: boot
an iPhone in Apple's Simulator app and choose **iPhone Simulator (POC)** from
a pane's `+` menu.

The experiment is intentionally narrow:

- It never creates, boots, shuts down, installs to, or launches anything on a
  simulated device.
- It discovers devices through the active Xcode's `simctl`, then attaches the
  corresponding CoreSimulator device to SimulatorKit's native display view.
- It accepts only Apple-signed CoreSimulator and SimulatorKit binaries and
  validates the private classes, selectors, method encodings, and NSView
  inheritance before sending private messages.
- The framebuffer connection uses SimulatorKit's private Swift ABI. It is
  restricted to Apple silicon and the exact SimulatorKit 1005.2 / CoreSimulator
  1171.7 pair; every other version is rejected before the bridge is called.
- The tab and its native objects are process-local and are not persisted.
- There is only one simulator tab per ket process.

This is not an App Store-compatible or supported Apple integration. Private
frameworks can change with any Xcode release, so incompatibility is reported
inside the tab instead of attempting a fallback ABI. The POC was initially
probed against Xcode 27.0 (SimulatorKit 1005.2, CoreSimulator 1171.7).
