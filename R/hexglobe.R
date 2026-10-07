#' @useDynLib hexglobe, .registration = TRUE
#' @keywords internal
"_PACKAGE"

#' Draw a globe scene
#'
#' Draws a scene on the graphics card, offscreen, with the shader and the
#' pipelines of the 'hexify' globe widget. Scenes are built by 'hexify' from
#' a globe widget; \code{hexify::hex_globe_png()} is the usual way in.
#'
#' A scene is a list of
#' \describe{
#'   \item{shader}{the WGSL source, one string;}
#'   \item{camera}{the camera uniform, 28 32-bit floats as a raw vector;}
#'   \item{width, height}{the image size in pixels;}
#'   \item{layers}{the layers in drawing order, each a list with \code{kind}
#'     (0 an opaque mesh, 1 a blended mesh coloured by value, 2 the cells of
#'     an ISEA grid, 3 polylines), \code{count} (indices of a mesh, segments
#'     of a polyline), and its buffers as raw vectors: \code{uniform},
#'     \code{pos}, \code{item}, \code{tri}, \code{index}, \code{values},
#'     \code{ramp}, \code{grid}, \code{keys}, \code{points},
#'     \code{segment}.}
#' }
#'
#' @param scene A scene, as above.
#'
#' @return An image: a list of \code{rgba}, the pixels as a raw vector of
#'   red, green, blue and alpha bytes, row by row from the top, alpha not
#'   premultiplied, and \code{width} and \code{height}.
#'
#' @export
render_scene <- function(scene) {
  image(.Call(C_render, scene))
}

#' The graphics adapter scenes are drawn on
#'
#' The adapter is picked once per session, preferring a discrete graphics
#' card. The environment variables \code{WGPU_POWER_PREF} (\code{"low"} or
#' \code{"high"}) and \code{WGPU_BACKEND} (such as \code{"dx12"},
#' \code{"vulkan"} or \code{"metal"}), set before the first scene is drawn,
#' choose another.
#'
#' @return The adapter's name and graphics backend, one string.
#' @export
gpu_adapter <- function() {
  .Call(C_adapter)
}

#' Write an image as a PNG file
#'
#' @param image An image from \code{\link{render_scene}} or
#'   \code{\link{read_png}}.
#' @param file Path of the PNG file to write.
#'
#' @return \code{file}, invisibly.
#' @export
write_png <- function(image, file) {
  png <- .Call(C_png_encode, image$rgba, image$width, image$height)
  writeBin(png$bytes, file)
  invisible(file)
}

#' Read a PNG file
#'
#' @param file Path of an 8-bit PNG file.
#'
#' @return An image, as from \code{\link{render_scene}}.
#' @export
read_png <- function(file) {
  image(.Call(C_png_decode, readBin(file, "raw", file.size(file))))
}

image <- function(x) {
  list(rgba = x$bytes, width = x$width, height = x$height)
}
