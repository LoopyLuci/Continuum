# Vendored from aurora-vnc

The three portable crates of `aurora-vnc`, so that Continuum is a single
self-contained build rather than a program that only compiles when a particular
checkout happens to sit at a particular absolute path on one machine.

## Why they are here

They were originally referenced as `path = "C:/Projects/aurora-vnc-main/..."`.
That is not a dependency, it is a hard-coded location on one developer's disk.
It cannot resolve on a CI runner, on a colleague's machine, or after a checkout
moves, which meant the integration existed only on the machine that made it.

Vendoring is the alternative that needs no network and no remote: the crates are
ordinary workspace members, built and tested by the same `cargo` invocation as
everything else in Continuum.

## What was changed on vendoring

Only the manifests, and only to stop them inheriting a workspace that is no
longer present:

* `edition`, `license`, `version` and `rust-version` were inlined. They came
  from aurora's root `[workspace.package]`, which does not exist here.
* `rfb-proto`'s dev-dependency on the `interop` crate was dropped, along with
  the tests that need it. `interop` drives TigerVNC's `Xvnc` and `x11vnc` over
  Xvfb, so it is Linux/X11-only and cannot be a dependency of a crate that
  builds on Windows. Removing it keeps `cargo test --workspace` working on the
  platform CI runs.
* Nothing in `src/` was changed to make it compile. If a source file needs
  changing, that is a real change with a real reason, and it should be reviewed
  as one.

## Provenance and divergence

These are copies, so they can drift from upstream. Two consequences worth
stating plainly:

* `vendor/aurora-vnc` is where defects in the vendored code get fixed. The
  Hextile encoder here places foreground pixels at their compacted index and
  corrupts any tile with a background pixel before a foreground one; the RRE
  encoder emits a sub-rectangle per differing pixel instead of per run; and
  `TightEncoder` keeps zlib streams alive across rectangles, which does not fit
  a server emitting self-contained per-rectangle payloads. All three were found
  by cross-checking Continuum's encoders against these, and Continuum keeps its
  own implementations until the vendored ones agree.
* If upstream is later published to a registry, swapping `vendor/aurora-vnc`
  for a registry dependency is a manifest-only change and these fixes would need
  porting forward.

## Keeping the conformance story

`cargo test --workspace` covers these crates like any other. The cross-checks
that matter are not their unit tests but Continuum's, in
`src/continuum-transport/tests/`, which decode what these encode with an
independent decoder and compare pixels.