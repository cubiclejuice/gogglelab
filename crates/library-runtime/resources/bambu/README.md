# Bambu printer CA bundle

`printer.cer` is the public CA bundle from `bambulab/BambuStudio`,
`resources/cert/printer.cer`. The GitHub Contents API response fetched on
2026-10-15 identified Git blob `6d8dbcb39bdc068c9adac668193fba6a6b85b1d5`,
was 6,540 bytes, and had SHA-256
`36f2bcee347ec7adce719b5fd350099591a4d3d0ec4e039c7019890d78e152a0`.

At runtime, GoggleLab pins the bundle hash and the fingerprints of its three
self-issued CA certificates. The two cross-signed CA certificates are supplied
only as candidate intermediates; they are not trust anchors. Changing this
asset requires a fresh provenance check and updates to the fingerprint and
chain-validation tests.

The bundle contains no printer leaf certificate. It does not establish the
certificate chain or serial/SAN behavior of any printer firmware family.
Unknown, expired, or name-mismatched chains must continue to fail closed.
