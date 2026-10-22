# Story: hexglobe

## Claim
Draws hexify's globe widgets offscreen on the graphics card through wgpu and returns PNG images, with no browser.

## Audience
hexify users who want globe images from scripts, reports or batch jobs; they know hexify's hex_globe().

## Relation
- to: hexify (parent; supplies the scenes and the shader)
- phrase: draws the globes of hexify
- same: shader and pipelines of the hexify globe widget
- differs: offscreen, on the graphics card, no browser
- home: hexify's Interactive Globe article

## Arcs

### A1 Globe images without a browser
- question: can I save a globe as an image from a script?
- answer: hexify::hex_globe_png() draws through hexglobe
- evidence: tests/testthat/test-hexglobe.R
- home: README Usage
- also: hexify Interactive Globe article

### A2 Which graphics card draws it
- question: what hardware and backend does it use, and can I change it?
- answer: Vulkan, Direct3D 12 or Metal; gpu_adapter(); WGPU_POWER_PREF, WGPU_BACKEND
- home: ?gpu_adapter
- also: README

## Non-goals
Interactive display (the hexify widget does that).

## Documents
- DESCRIPTION: claim, relation
- README: claim, relation, install with Rust, first globe, routing
- help pages: reference

## Open
- Repository is private; the pak install line works only with access.
