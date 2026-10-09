# tests/testthat/test-hexglobe.R

test_that("an image survives a PNG round trip", {
  rgba <- as.raw(c(255, 0, 0, 255, 0, 128, 255, 64, 1, 2, 3, 0,
                   10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120))
  image <- list(rgba = rgba, width = 3L, height = 2L)
  file <- tempfile(fileext = ".png")
  on.exit(unlink(file))
  expect_equal(write_png(image, file), file)
  expect_equal(readBin(file, "raw", 4)[2:4], charToRaw("PNG"))
  expect_equal(read_png(file), image)
})

test_that("an image of the wrong size is refused", {
  expect_error(write_png(list(rgba = raw(12), width = 2L, height = 2L), tempfile()),
               "width x height")
  expect_error(read_png(textConnection("x")))
})

skip_without_adapter <- function() {
  skip_on_cran()
  skip_if(inherits(try(gpu_adapter(), silent = TRUE), "try-error"),
          "no graphics adapter")
}

f32 <- function(x) writeBin(as.numeric(x), raw(), size = 4L, endian = "little")

# A shader with the entry points and bindings the renderer builds its four
# pipelines from; every vertex stage reads the camera, as globe.wgsl does.
shader <- "
struct Camera { a: vec4f, b: vec4f, c: vec4f, d: vec4f, e: vec4f, f: vec4f, g: vec4f };
struct Layer { color: vec4f, params: vec4f, fade: vec4f };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> layer: Layer;
@group(1) @binding(1) var<storage, read> values: array<f32>;
@group(1) @binding(2) var<storage, read> ramp: array<vec4f>;
@group(1) @binding(3) var<storage, read> points: array<f32>;
@group(1) @binding(4) var<uniform> grid: vec4f;
@group(1) @binding(6) var words: texture_2d_array<u32>;
@vertex fn vs_mesh(@location(0) solid: vec3f, @location(1) sphere: vec3f,
                   @location(2) item: u32) -> @builtin(position) vec4f {
  return vec4f(solid.xy * camera.a.x, 0.5, 1.0);
}
@fragment fn fs_mesh() -> @location(0) vec4f {
  return vec4f(layer.color.rgb * layer.color.a, layer.color.a) + 0.0 * values[0] * ramp[0];
}
@vertex fn vs_grid(@location(0) solid: vec3f, @location(1) sphere: vec3f,
                   @location(2) face: u32, @location(3) tri: vec2f) -> @builtin(position) vec4f {
  return vec4f(solid.xy * camera.a.x, 0.5, 1.0);
}
@fragment fn fs_grid() -> @location(0) vec4f {
  return grid + 0.0 * ramp[0] * f32(textureLoad(words, vec2u(0u, 0u), 0u, 0).x) * layer.color;
}
@vertex fn vs_line(@builtin(vertex_index) vi: u32, @location(0) start: u32) -> @builtin(position) vec4f {
  return vec4f(points[start] * camera.a.x, 0.0, 0.5, 1.0);
}
@fragment fn fs_line() -> @location(0) vec4f {
  return layer.color;
}
"

test_that("a scene is drawn at its size, straight alpha, rows from the top", {
  skip_without_adapter()
  # One triangle over the top half of the image, half transparent orange.
  pos <- f32(c(-1, 0, 0, 0, 0, 0,
                3, 0, 0, 0, 0, 0,
               -1, 4, 0, 0, 0, 0))
  layer <- list(kind = 1L, count = 3L,
                uniform = f32(c(1, 0.5, 0, 0.5, rep(0, 8))),
                pos = pos, item = writeBin(integer(3), raw(), size = 4L),
                index = writeBin(0:2, raw(), size = 4L),
                values = f32(0), ramp = f32(numeric(4)))
  scene <- list(shader = shader, camera = f32(c(1, rep(0, 27))),
                width = 8L, height = 6L, layers = list(layer))
  img <- render_scene(scene)
  expect_equal(c(img$width, img$height), c(8L, 6L))
  px <- array(as.integer(img$rgba), c(4, 8, 6))
  # 8-bit premultiplied alpha: 0.5 is stored as 127, green as 64 of it.
  expect_lte(max(abs(px[, 4, 1] - c(255, 128, 0, 128))), 1)
  expect_equal(px[, 4, 6], c(0, 0, 0, 0))
  expect_type(gpu_adapter(), "character")
})

test_that("a malformed scene is refused with the renderer's reason", {
  skip_without_adapter()
  scene <- list(shader = shader, camera = f32(1:4), width = 4L, height = 4L,
                layers = list())
  expect_error(render_scene(scene), "28 floats")
  scene$camera <- f32(numeric(28))
  scene$layers <- list(list(kind = 9L, count = 0L))
  expect_error(render_scene(scene), "kind 9")
  scene$layers <- list()
  scene$shader <- "not wgsl"
  expect_error(render_scene(scene), "refused")
})
