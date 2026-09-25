# Third-party notices

CigScript's binary links the Rust crates below, pulled from crates.io as packages (nothing is vendored). Every one is under a permissive licence compatible with Apache-2.0. Holder names come from each crate's metadata; `unrecorded` means the crate publishes no author field. Regenerate with `cargo license --avoid-dev-deps --json` (cargo-license) and `scripts/gen-third-party.py`.

## MIT OR Unlicense

| crate | version | holder | source |
|---|---|---|---|
| aho-corasick | 1.1.5 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/aho-corasick |
| globset | 0.4.20 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/ripgrep/tree/master/crates/globset |
| memchr | 2.8.3 | Andrew Gallant <jamslam@gmail.com>, bluss | https://github.com/BurntSushi/memchr |
| same-file | 1.0.6 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/same-file |
| walkdir | 2.5.0 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/walkdir |
| winapi-util | 0.1.11 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/winapi-util |

## Apache-2.0 OR MIT

| crate | version | holder | source |
|---|---|---|---|
| android_system_properties | 0.1.6 | Nicolas Silva <nical@fastmail.com> | https://github.com/nical/android_system_properties |
| anstream | 1.0.0 | unrecorded | https://github.com/rust-cli/anstyle.git |
| anstyle | 1.0.14 | unrecorded | https://github.com/rust-cli/anstyle.git |
| anstyle-parse | 1.0.0 | unrecorded | https://github.com/rust-cli/anstyle.git |
| anstyle-query | 1.1.5 | unrecorded | https://github.com/rust-cli/anstyle.git |
| anstyle-wincon | 3.0.11 | unrecorded | https://github.com/rust-cli/anstyle.git |
| autocfg | 1.5.1 | Josh Stone <cuviper@gmail.com> | https://github.com/cuviper/autocfg |
| block-buffer | 0.10.4 | RustCrypto Developers | https://github.com/RustCrypto/utils |
| bstr | 1.13.1 | Andrew Gallant <jamslam@gmail.com> | https://github.com/BurntSushi/bstr |
| bumpalo | 3.20.3 | Nick Fitzgerald <fitzgen@gmail.com> | https://github.com/fitzgen/bumpalo |
| cc | 1.5.1 | unrecorded | https://github.com/rust-lang/cc-rs |
| cfg-if | 1.0.5 | Alex Crichton <alex@alexcrichton.com> | https://github.com/rust-lang/cfg-if |
| chrono | 0.4.45 | unrecorded | https://github.com/chronotope/chrono |
| clap | 4.6.7 | unrecorded | https://github.com/clap-rs/clap |
| clap_builder | 4.6.7 | unrecorded | https://github.com/clap-rs/clap |
| clap_derive | 4.6.7 | unrecorded | https://github.com/clap-rs/clap |
| clap_lex | 1.1.1 | unrecorded | https://github.com/clap-rs/clap |
| colorchoice | 1.0.5 | unrecorded | https://github.com/rust-cli/anstyle.git |
| core-foundation-sys | 0.8.7 | The Servo Project Developers | https://github.com/servo/core-foundation-rs |
| cpufeatures | 0.2.17 | RustCrypto Developers | https://github.com/RustCrypto/utils |
| crypto-common | 0.1.7 | RustCrypto Developers | https://github.com/RustCrypto/traits |
| digest | 0.10.7 | RustCrypto Developers | https://github.com/RustCrypto/traits |
| equivalent | 1.0.2 | unrecorded | https://github.com/indexmap-rs/equivalent |
| find-msvc-tools | 0.1.14 | unrecorded | https://github.com/rust-lang/cc-rs |
| futures-core | 0.3.34 | unrecorded | https://github.com/rust-lang/futures-rs |
| futures-task | 0.3.34 | unrecorded | https://github.com/rust-lang/futures-rs |
| futures-util | 0.3.34 | unrecorded | https://github.com/rust-lang/futures-rs |
| hashbrown | 0.17.1 | unrecorded | https://github.com/rust-lang/hashbrown |
| heck | 0.5.0 | unrecorded | https://github.com/withoutboats/heck |
| hex | 0.4.3 | KokaKiwi <kokakiwi@kokakiwi.net> | https://github.com/KokaKiwi/rust-hex |
| iana-time-zone | 0.1.65 | Andrew Straw <strawman@astraw.com>, René Kijewski <rene.kijewski@fu-berlin.de>, Ryan Lopopolo <rjl@hyperbo.la> | https://github.com/strawlab/iana-time-zone |
| iana-time-zone-haiku | 0.1.2 | René Kijewski <crates.io@k6i.de> | https://github.com/strawlab/iana-time-zone |
| indexmap | 2.14.2 | unrecorded | https://github.com/indexmap-rs/indexmap |
| is_terminal_polyfill | 1.70.2 | unrecorded | https://github.com/polyfill-rs/is_terminal_polyfill |
| itoa | 1.0.18 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/itoa |
| js-sys | 0.3.106 | The wasm-bindgen Developers | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys |
| libc | 0.2.189 | unrecorded | https://github.com/rust-lang/libc |
| log | 0.4.34 | The Rust Project Developers | https://github.com/rust-lang/log |
| num-traits | 0.2.19 | The Rust Project Developers | https://github.com/rust-num/num-traits |
| once_cell | 1.21.4 | Aleksey Kladov <aleksey.kladov@gmail.com> | https://github.com/matklad/once_cell |
| once_cell_polyfill | 1.70.2 | unrecorded | https://github.com/polyfill-rs/once_cell_polyfill |
| pin-project-lite | 0.2.17 | unrecorded | https://github.com/taiki-e/pin-project-lite |
| proc-macro2 | 1.0.107 | David Tolnay <dtolnay@gmail.com>, Alex Crichton <alex@alexcrichton.com> | https://github.com/dtolnay/proc-macro2 |
| quote | 1.0.47 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/quote |
| regex | 1.13.1 | The Rust Project Developers, Andrew Gallant <jamslam@gmail.com> | https://github.com/rust-lang/regex |
| regex-automata | 0.4.18 | The Rust Project Developers, Andrew Gallant <jamslam@gmail.com> | https://github.com/rust-lang/regex |
| regex-syntax | 0.8.11 | The Rust Project Developers, Andrew Gallant <jamslam@gmail.com> | https://github.com/rust-lang/regex |
| rustversion | 1.0.23 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/rustversion |
| serde | 1.0.229 | Erick Tryzelaar <erick.tryzelaar@gmail.com>, David Tolnay <dtolnay@gmail.com> | https://github.com/serde-rs/serde |
| serde_core | 1.0.229 | Erick Tryzelaar <erick.tryzelaar@gmail.com>, David Tolnay <dtolnay@gmail.com> | https://github.com/serde-rs/serde |
| serde_derive | 1.0.229 | Erick Tryzelaar <erick.tryzelaar@gmail.com>, David Tolnay <dtolnay@gmail.com> | https://github.com/serde-rs/serde |
| serde_json | 1.0.151 | Erick Tryzelaar <erick.tryzelaar@gmail.com>, David Tolnay <dtolnay@gmail.com> | https://github.com/serde-rs/json |
| sha2 | 0.10.9 | RustCrypto Developers | https://github.com/RustCrypto/hashes |
| shlex | 2.0.1 | comex <comexk@gmail.com>, Fenhl <fenhl@fenhl.net>, Adrian Taylor <adetaylor@chromium.org>, Alex Touchet <alextouchet@outlook.com>, Daniel Parks <dp+git@oxidized.org>, Garrett Berg <googberg@gmail.com> | https://github.com/comex/rust-shlex |
| syn | 2.0.119 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/syn |
| syn | 3.0.6 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/syn |
| typenum | 1.20.1 | unrecorded | https://github.com/paholg/typenum |
| utf8parse | 0.2.2 | Joe Wilm <joe@jwilm.com>, Christian Duerr <contact@christianduerr.com> | https://github.com/alacritty/vte |
| version_check | 0.9.5 | Sergio Benitez <sb@sergio.bz> | https://github.com/SergioBenitez/version_check |
| wait-timeout | 0.2.1 | Alex Crichton <alex@alexcrichton.com> | https://github.com/alexcrichton/wait-timeout |
| wasm-bindgen | 0.2.129 | The wasm-bindgen Developers | https://github.com/wasm-bindgen/wasm-bindgen |
| wasm-bindgen-macro | 0.2.129 | The wasm-bindgen Developers | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro |
| wasm-bindgen-macro-support | 0.2.129 | The wasm-bindgen Developers | https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/macro-support |
| wasm-bindgen-shared | 0.2.129 | The wasm-bindgen Developers | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared |
| windows-core | 0.62.2 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-implement | 0.60.2 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-interface | 0.59.3 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-link | 0.2.1 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-result | 0.4.1 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-strings | 0.5.1 | unrecorded | https://github.com/microsoft/windows-rs |
| windows-sys | 0.61.2 | unrecorded | https://github.com/microsoft/windows-rs |

## MIT

| crate | version | holder | source |
|---|---|---|---|
| generic-array | 0.14.7 | Bartłomiej Kamiński <fizyk20@gmail.com>, Aaron Trent <novacrazy@gmail.com> | https://github.com/fizyk20/generic-array.git |
| slab | 0.4.12 | Carl Lerche <me@carllerche.com> | https://github.com/tokio-rs/slab |
| strsim | 0.11.1 | Danny Guo <danny@dannyguo.com>, maxbachmann <oss@maxbachmann.de> | https://github.com/rapidfuzz/strsim-rs |
| zmij | 1.0.23 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/zmij |

## (Apache-2.0 OR MIT) AND Unicode-3.0

| crate | version | holder | source |
|---|---|---|---|
| unicode-ident | 1.0.26 | David Tolnay <dtolnay@gmail.com> | https://github.com/dtolnay/unicode-ident |
