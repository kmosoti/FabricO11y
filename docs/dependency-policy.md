# Dependency license policy

The release workspace includes the server and Leptos/WASM console. Dependency
policy must inspect that complete graph; a root-package-only graph misses
dependencies of these applications. Advisory exclusions remain empty.

The allowlist admits the following additional permissive licenses after review
of their canonical texts. This is a policy change, to be committed separately
from dependency implementation changes; it is not a crate-specific exception.

| SPDX identifier | Current consumers prompting review | Permissions and obligations |
| --- | --- | --- |
| BSL-1.0 | xxhash-rust | The [Boost Software License](https://www.boost.org/LICENSE_1_0.txt) grants use, modification and distribution. Preserve its copyright and license statement in source copies and derivatives; its text exempts solely machine-executable object code. This identifier means Boost Software License, not Business Source License. |
| Zlib | slotmap; the const_format/konst family | The [zlib license](https://www.zlib.net/zlib_license.html) permits commercial use, alteration and redistribution. Do not misrepresent origin, clearly mark altered source versions, and retain its notice in source distributions. |
| CC0-1.0 | base16; tiny-keccak | [CC0's legal text](https://creativecommons.org/publicdomain/zero/1.0/legalcode) waives copyright and related rights, with a permissive fallback license where the waiver is ineffective. It imposes no source-disclosure requirement. It grants no patent or trademark rights and disclaims warranties. |

These grants are consistent with the repository's permissive dependency policy.
Allowlisting does not eliminate their notice obligations or establish patent
clearance. Preserve upstream license files in local patches and source archives;
package/source distribution review must retain the notices required by the
selected licenses. Existing MIT, Apache and other license obligations continue
to apply.

`rustls-pemfile` is separately addressed by upgrading axum-server to 0.8.0.
Its [upstream changelog](https://docs.rs/crate/axum-server/0.8.0/source/CHANGELOG.md)
records replacement by rustls-pki-types, rather than advisory suppression.
The [local UI patches](../vendor/README.md) replace unmaintained paste with
the explicitly named pastey registry package.

License-text review supports the allowlist decision. Only a recorded
`cargo deny --workspace --locked check` run can establish the corresponding
dependency gate result for a particular revision and lockfile.
