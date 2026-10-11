# Third-party notices

BuddyHub's own source is MIT-licensed. Third-party components remain under their original licenses.

- The Windows desktop build includes Microsoft .NET and Windows Forms, distributed under the MIT license and third-party notices provided by Microsoft. Runtime notice files are bundled under `licenses/` in downloadable releases.
- Rust dependency versions are recorded in `native/Cargo.lock`. The build generates `licenses/rust-dependencies.md` and copies upstream license/notice files into `licenses/rust/`.
- The application icon and its SVG source are supplied with this project under the project license. WorkBuddy and other product names are used only to identify compatible products.

The release's `licenses/` directory is generated from the source dependency packages used in that build; it is part of the distribution.
