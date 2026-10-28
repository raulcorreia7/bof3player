# Changelog

All notable changes to `psx-cd` are recorded here. The crate follows
[Semantic Versioning](https://semver.org/).

## 0.1.0

First release, as its own crate.

- Images: cooked 2048-byte ISO9660 and raw 2352-byte Mode 1/Mode 2 images,
  opened from a path, a CUE sheet or any `Read + Seek` source.
- File table: directory records with both-endian validation, file and directory
  paths, extended attributes and CD-XA attributes.
- Reads: file bytes, random ranges, 2336-byte XA sectors and the license
  sectors of a raw image.
- Extraction: the file tree, the license sectors and the CD-DA tracks.
- CUE: `FILE`, `TRACK`, `INDEX` and `PREGAP` parsing, sector ranges, and CD-DA
  extraction as WAV or raw PCM.
- Checksums: MD5 and SHA-256 manifests with write, read and verify.
- Metadata: project XML and `disc_lba.json`.
- Patching: replace files in place, update their directory records and re-seal
  every changed Mode 2 Form 1 sector.
- Rebuild: a deterministic cooked ISO9660 image from a folder of files.
