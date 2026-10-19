# iced_baseview

[![Documentation](https://docs.rs/iced_baseview/badge.svg)](https://docs.rs/iced_baseview)
[![Crates.io](https://img.shields.io/crates/v/iced_baseview.svg)](https://crates.io/crates/iced_baseview)
[![License](https://img.shields.io/crates/l/iced_baseview.svg)](https://codeberg.org/RustAudio/iced_baseview/src/branch/main/LICENSE-APACHE)

A [`baseview`](https://github.com/RustAudio/baseview) backend for [`Iced`](https://iced.rs/)

This is used by the [nice-plug](https://codeberg.org/RustAudio/nice-plug) framework, but it can also be used in your own custom audio plugin framework (i.e. with [clack-plugin](https://crates.io/crates/clack-plugin)).

<div align="center">
    <img src="screenshot.png">
</div>

## Prerequisites

### Linux

Install dependencies, e.g.,

```sh
sudo apt-get install libx11-dev libxcursor-dev libxcb-dri2-0-dev libxcb-icccm4-dev libx11-xcb-dev mesa-common-dev libgl1-mesa-dev libglu1-mesa-dev
```

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0), or
* MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.
