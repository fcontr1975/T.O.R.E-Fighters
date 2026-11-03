# Third-party notices

## PKWare DCL decoding

The Rust raw-literal DCL decoder in `crates/tore-formats/src/dcl.rs` uses the
format tables, canonical decoding method, and public test vector documented by
Mark Adler's [blast](https://github.com/madler/zlib/tree/master/contrib/blast),
also consulted through the local USNF-ATF reference decoder. This is an altered,
bounded Rust implementation, not the original blast distribution. It supports
raw literals only, limits output, and returns Rust I/O errors.

The upstream notice from `blast.h` follows:

```text
Copyright (C) 2003, 2012, 2013 Mark Adler

This software is provided 'as-is', without any express or implied
warranty.  In no event will the author be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.
3. This notice may not be removed or altered from any source distribution.

Mark Adler    madler@alumni.caltech.edu
```

## Local game data and reference specifications

Game art, fonts, sounds, and other retail resources remain user-supplied. They
are not licensed or distributed by this repository. Format observations were
checked against the local `john-overton/USNF-ATF` checkout and Fighters Anthology
media; see `docs/formats/menu.md` for provenance and validation scope.

Rust dependency licenses remain those published by their respective authors.
`Cargo.lock` records the versions used.

## zune-jpeg

The loading photographs are decoded with
[zune-jpeg](https://github.com/etemesi254/zune-image) 0.5.15 and its
`zune-core` dependency. Both are MIT OR Apache-2.0 OR Zlib. This build turns
default features off, so the decode is the scalar path.

## naga

`crates/vendor/naga` is [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga)
27.0.3, MIT OR Apache-2.0, altered as `crates/vendor/naga/PATCHED.md` describes.
Both license texts are in that folder.

## OpenFA export tooling references

`tools/openfa/upstream/sh.rs` and `lib_ext.rs` are unmodified OpenFA 0.2.14
command sources at revision `7507fef5bbb126302a59cb413e80cadf5c547f9d`,
from https://gitlab.com/openfa/openfa. They retain their GPL notices; the full
GPL-3.0 license and source hashes are alongside them. The local static-export
patch is a T.O.R.E agent-authored modification, not an upstream release.
OpenFA is built separately under `.local/`, not linked into the game.

## Release-check dependencies

The app's background release check uses [ureq](https://github.com/algesten/ureq)
3.5.0 (MIT OR Apache-2.0), with default features disabled and `rustls` enabled.
[serde_json](https://github.com/serde-rs/json) 1.0.154 and
[semver](https://github.com/dtolnay/semver) 1.0.28 are MIT OR Apache-2.0.
`Cargo.lock` is the version authority. Transitive additions are:

| Packages | Licence |
| --- | --- |
| ureq-proto, base64, getrandom, http, httparse, itoa, rustls-pki-types, utf8-zero | MIT OR Apache-2.0 |
| rustls | Apache-2.0 OR ISC OR MIT |
| ring | Apache-2.0 AND ISC |
| rustls-webpki, untrusted | ISC |
| subtle | BSD-3-Clause |
| webpki-roots (Mozilla trust anchors) | CDLA-Permissive-2.0 |
| zeroize | Apache-2.0 OR MIT |
| zmij | MIT |
| wasi (target-specific getrandom dependency) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |

ring includes upstream cryptographic source notices in its
[LICENSE](https://github.com/briansmith/ring/blob/0.17.14/LICENSE).
The other packages retain their upstream licence texts in their crate source
archives. No OpenSSL or other system TLS library is linked by this feature.
