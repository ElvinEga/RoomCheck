//! Standalone host for quickly auditioning Roomcheck without a DAW.
//! Build a plugin bundle with `cargo xtask bundle roomcheck --release`.

fn main() {
    nice_plug::prelude::nice_export_standalone::<roomcheck::Roomcheck>();
}
