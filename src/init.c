// init.c
// R's side of the renderer: R lists and raw vectors in, the C ABI of
// src/rust/lib.rs called, its bytes copied into R and released.

#include <stdint.h>
#include <stdio.h>
#include <stddef.h>
#include <string.h>
#include <R.h>
#include <Rinternals.h>
#include <R_ext/Rdynload.h>

typedef struct {
  const uint8_t *ptr;
  size_t len;
} HgBuf;

typedef struct {
  uint32_t binding;
  uint32_t format;
  uint32_t dimension;
  uint32_t width, height, layers;
  HgBuf data;
} HgTexture;

typedef struct {
  uint32_t kind;
  uint32_t count;
  HgBuf uniform, pos, item, tri, index, values, ramp, grid, points, segment;
  const HgTexture *textures;
  size_t n_textures;
} HgLayer;

typedef struct {
  HgBuf shader;
  HgBuf camera;
  uint32_t width;
  uint32_t height;
  const HgLayer *layers;
  size_t n_layers;
} HgScene;

typedef struct {
  uint8_t *data;
  size_t len;
  uint32_t width;
  uint32_t height;
  char *error;
} HgResult;

extern int hexglobe_render(const HgScene *scene, HgResult *out);
extern int hexglobe_adapter(HgResult *out);
extern int hexglobe_png_encode(HgBuf rgba, uint32_t width, uint32_t height, HgResult *out);
extern int hexglobe_png_decode(HgBuf file, HgResult *out);
extern void hexglobe_free(HgResult *res);

// The element `name` of list `x`, or R_NilValue.
static SEXP get(SEXP x, const char *name) {
  SEXP names = Rf_getAttrib(x, R_NamesSymbol);
  for (R_xlen_t i = 0; i < Rf_xlength(x); i++) {
    if (strcmp(CHAR(STRING_ELT(names, i)), name) == 0) return VECTOR_ELT(x, i);
  }
  return R_NilValue;
}

// A raw vector's bytes; NULL leaves the buffer empty.
static HgBuf buf(SEXP x, const char *what) {
  HgBuf b = {NULL, 0};
  if (Rf_isNull(x)) return b;
  if (TYPEOF(x) != RAWSXP) Rf_error("%s must be a raw vector", what);
  b.ptr = RAW(x);
  b.len = (size_t) XLENGTH(x);
  return b;
}

static uint32_t count(SEXP x, const char *what) {
  if (!Rf_isNumeric(x) || Rf_xlength(x) != 1) Rf_error("%s must be a single number", what);
  double v = Rf_asReal(x);
  if (!(v >= 0 && v <= 4294967295.0)) Rf_error("%s is out of range", what);
  return (uint32_t) v;
}

// One of `options`, by name, as its position
static uint32_t choice(SEXP x, const char *const *options, int n, const char *what) {
  if (TYPEOF(x) != STRSXP || Rf_xlength(x) != 1) Rf_error("%s must be a single string", what);
  const char *s = CHAR(STRING_ELT(x, 0));
  for (int k = 0; k < n; k++) {
    if (strcmp(s, options[k]) == 0) return (uint32_t) k;
  }
  Rf_error("%s '%s' is not one the renderer knows", what, s);
  return 0;
}

static const char *const FORMATS[] = {"r32uint", "rg32uint"};
static const char *const DIMENSIONS[] = {"2d", "2d-array"};

// A layer's textures: each a list of binding, format, dimension, size
// (width, height, layers) and data.
static void textures(SEXP x, HgLayer *o) {
  o->textures = NULL;
  o->n_textures = 0;
  if (Rf_isNull(x)) return;
  if (TYPEOF(x) != VECSXP) Rf_error("a layer's textures must be a list");
  R_xlen_t n = Rf_xlength(x);
  HgTexture *ts = (HgTexture *) R_alloc(n > 0 ? (size_t) n : 1, sizeof(HgTexture));
  for (R_xlen_t i = 0; i < n; i++) {
    SEXP t = VECTOR_ELT(x, i);
    if (TYPEOF(t) != VECSXP) Rf_error("every texture must be a list");
    ts[i].binding = count(get(t, "binding"), "a texture's binding");
    ts[i].format = choice(get(t, "format"), FORMATS, 2, "a texture's format");
    ts[i].dimension = choice(get(t, "dimension"), DIMENSIONS, 2, "a texture's dimension");
    SEXP size = get(t, "size");
    if (!Rf_isNumeric(size) || Rf_xlength(size) != 3) Rf_error("a texture's size must be three numbers");
    SEXP sz = PROTECT(Rf_coerceVector(size, REALSXP));
    double *d = REAL(sz);
    for (int k = 0; k < 3; k++) {
      if (!(d[k] >= 1 && d[k] <= 4294967295.0)) Rf_error("a texture's size is out of range");
    }
    ts[i].width = (uint32_t) d[0];
    ts[i].height = (uint32_t) d[1];
    ts[i].layers = (uint32_t) d[2];
    UNPROTECT(1);
    ts[i].data = buf(get(t, "data"), "a texture's data");
  }
  o->textures = ts;
  o->n_textures = (size_t) n;
}

// The result's bytes as a list (bytes, width, height), or R's error with
// the renderer's message.
static SEXP take(int status, HgResult *res) {
  if (status != 0) {
    char msg[1024];
    snprintf(msg, sizeof msg, "%s", res->error ? res->error : "unknown error");
    hexglobe_free(res);
    Rf_error("%s", msg);
  }
  SEXP bytes = PROTECT(Rf_allocVector(RAWSXP, (R_xlen_t) res->len));
  if (res->len) memcpy(RAW(bytes), res->data, res->len);
  SEXP out = PROTECT(Rf_allocVector(VECSXP, 3));
  SEXP names = PROTECT(Rf_allocVector(STRSXP, 3));
  SET_VECTOR_ELT(out, 0, bytes);
  SET_VECTOR_ELT(out, 1, Rf_ScalarInteger((int) res->width));
  SET_VECTOR_ELT(out, 2, Rf_ScalarInteger((int) res->height));
  SET_STRING_ELT(names, 0, Rf_mkChar("bytes"));
  SET_STRING_ELT(names, 1, Rf_mkChar("width"));
  SET_STRING_ELT(names, 2, Rf_mkChar("height"));
  Rf_setAttrib(out, R_NamesSymbol, names);
  hexglobe_free(res);
  UNPROTECT(3);
  return out;
}

static SEXP C_render(SEXP scene) {
  if (TYPEOF(scene) != VECSXP) Rf_error("scene must be a list");
  SEXP shader = get(scene, "shader");
  if (TYPEOF(shader) != STRSXP || Rf_xlength(shader) != 1) {
    Rf_error("scene$shader must be a single string");
  }
  SEXP layers = get(scene, "layers");
  if (TYPEOF(layers) != VECSXP) Rf_error("scene$layers must be a list");
  R_xlen_t n = Rf_xlength(layers);
  HgLayer *ls = (HgLayer *) R_alloc(n > 0 ? (size_t) n : 1, sizeof(HgLayer));
  for (R_xlen_t i = 0; i < n; i++) {
    SEXP l = VECTOR_ELT(layers, i);
    if (TYPEOF(l) != VECSXP) Rf_error("every layer must be a list");
    HgLayer *o = &ls[i];
    o->kind = count(get(l, "kind"), "a layer's kind");
    o->count = count(get(l, "count"), "a layer's count");
    o->uniform = buf(get(l, "uniform"), "a layer's uniform");
    o->pos = buf(get(l, "pos"), "a layer's pos");
    o->item = buf(get(l, "item"), "a layer's item");
    o->tri = buf(get(l, "tri"), "a layer's tri");
    o->index = buf(get(l, "index"), "a layer's index");
    o->values = buf(get(l, "values"), "a layer's values");
    o->ramp = buf(get(l, "ramp"), "a layer's ramp");
    o->grid = buf(get(l, "grid"), "a layer's grid");
    textures(get(l, "textures"), o);
    o->points = buf(get(l, "points"), "a layer's points");
    o->segment = buf(get(l, "segment"), "a layer's segment");
  }
  const char *code = CHAR(STRING_ELT(shader, 0));
  HgScene s;
  s.shader.ptr = (const uint8_t *) code;
  s.shader.len = strlen(code);
  s.camera = buf(get(scene, "camera"), "scene$camera");
  s.width = count(get(scene, "width"), "scene$width");
  s.height = count(get(scene, "height"), "scene$height");
  s.layers = ls;
  s.n_layers = (size_t) n;
  HgResult res;
  return take(hexglobe_render(&s, &res), &res);
}

static SEXP C_adapter(void) {
  HgResult res;
  SEXP out = PROTECT(take(hexglobe_adapter(&res), &res));
  SEXP bytes = VECTOR_ELT(out, 0);
  SEXP text = PROTECT(Rf_mkCharLenCE((const char *) RAW(bytes), (int) XLENGTH(bytes), CE_UTF8));
  SEXP s = Rf_ScalarString(text);
  UNPROTECT(2);
  return s;
}

static SEXP C_png_encode(SEXP rgba, SEXP width, SEXP height) {
  HgResult res;
  return take(hexglobe_png_encode(buf(rgba, "rgba"), count(width, "width"),
                                  count(height, "height"), &res), &res);
}

static SEXP C_png_decode(SEXP file) {
  HgResult res;
  return take(hexglobe_png_decode(buf(file, "the PNG bytes"), &res), &res);
}

static const R_CallMethodDef calls[] = {
  {"C_render", (DL_FUNC) &C_render, 1},
  {"C_adapter", (DL_FUNC) &C_adapter, 0},
  {"C_png_encode", (DL_FUNC) &C_png_encode, 3},
  {"C_png_decode", (DL_FUNC) &C_png_decode, 1},
  {NULL, NULL, 0}
};

void R_init_hexglobe(DllInfo *dll) {
  R_registerRoutines(dll, NULL, calls, NULL, NULL);
  R_useDynamicSymbols(dll, FALSE);
  R_forceSymbols(dll, TRUE);
}
