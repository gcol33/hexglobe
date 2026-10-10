# hexglobe

## Installation

Building needs Rust's `cargo` and `rustc` (<https://rustup.rs>); on Windows
also the `x86_64-pc-windows-gnu` target (`rustup target add
x86_64-pc-windows-gnu`), which R's toolchain links against.

```r
remotes::install_github("gcol33/hexglobe")
```

The renderer draws on Vulkan, Direct3D 12 or Metal, whichever the machine
has. `gpu_adapter()` names the graphics card in use; the environment
variables `WGPU_POWER_PREF` and `WGPU_BACKEND` choose another.

## Quick start

Renders the globes of [hexify](https://github.com/gcol33/hexify) on the
graphics card from R, with no browser. A globe from `hexify::hex_globe()` is
drawn offscreen through [wgpu](https://wgpu.rs), the Rust implementation of
WebGPU, with the widget's own shader and pipelines, and saved as a PNG:

```r
library(hexify)
grid <- hex_grid(resolution = 3, aperture = 3)
globe <- hex_globe(grid, surface = "solid", center = "pacific")
hex_globe_png(globe, "globe.png", scale = 2)
```

The image matches the widget's to within the anti-aliasing of edges, and a
globe takes a few hundredths of a second once the graphics device is open.
