# Reflex overlay

The overlay is the visible trust signal that ReflexDesk is active.

## Visual direction
A lightweight semi-transparent futuristic pulse/orb, centered near the lower middle of the screen by default.

It should feel like energy moving through glass, not a giant assistant window.

## States
- idle: hidden
- listening: soft breathing translucent pulse
- speech: waveform/ring responds to microphone RMS
- routing: tighter faster pulse
- acting: brief directional flash
- harness-working: slow orbiting particles/rings
- success: short clean confirmation collapse
- confirmation-required: amber warning pulse
- error: distinct interrupted/glitch pulse

## Interaction
- always on top while active
- click-through unless expanded
- no focus stealing
- draggable position when unlocked
- reduced-motion accessibility option
- opacity/intensity configurable
- can be disabled independently from voice control

## Global shortcut
Default:
- Windows/Linux: Ctrl+Shift+Space
- macOS: Cmd+Shift+Space

One tap toggles active listening.
Second tap stops capture immediately and clears pending simple-command recognition.

Push-to-talk is an optional mode.

The shortcut and listening behavior are user-configurable.

## Privacy invariant
If active capture is running, the overlay must be visible unless the user explicitly selected a separate accessibility indicator mode.
