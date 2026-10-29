# Vendored Taffy

This is Taffy 0.9.0 from crates.io, vendored because gpui 0.2.2 pins that version.
Its `grid` dependency is patched from 0.18.0 to 1.0.1 to resolve Dependabot alert 1.
Remove this patch when gpui updates Taffy to a version that uses a fixed `grid` release.
